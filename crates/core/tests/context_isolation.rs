//! Black-box state-binding and isolated-context lifecycle tests for CAS-315.
//!
//! Requires local Chromium. Run serially:
//!
//!     cargo test -p void_crawl_core --test context_isolation --
//! --test-threads=1
#![allow(
    clippy::absolute_paths,
    clippy::expect_used,
    clippy::implicit_clone,
    clippy::panic,
    clippy::unwrap_used
)]

use std::{
    collections::HashSet,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::Duration,
};

use tokio::{
    task::yield_now,
    time::{Instant, sleep, timeout},
};
use void_crawl_core::{
    BrowserPool, BrowserSession, BrowserStateBinding, ContextDisposalState, PoolConfig,
    PoolReleaseStrategy,
};

struct Fixture {
    url:    String,
    stop:   std::sync::mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let address = listener.local_addr().expect("fixture address");
        let (stop, stopped) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            loop {
                if stopped.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 4096];
                        let read = stream.read(&mut request).unwrap_or(0);
                        let request = String::from_utf8_lossy(&request[..read]);
                        let path = request.split_whitespace().nth(1).unwrap_or("/");
                        let (content_type, body) = if path == "/sw.js" {
                            ("application/javascript", "self.addEventListener('fetch', () => {});")
                        } else {
                            (
                                "text/html",
                                "<!doctype html><title>isolated fixture</title><main>fixture</main>",
                            )
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nService-Worker-Allowed: /\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            }
        });
        Self { url: format!("http://{address}/"), stop, thread: Some(thread) }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join fixture");
        }
    }
}

async fn session() -> BrowserSession {
    BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch Chromium")
}

#[tokio::test]
async fn session_close_before_reaps_the_launched_browser() {
    let browser = timeout(Duration::from_secs(20), BrowserSession::launch_headless())
        .await
        .expect("browser launch deadline")
        .expect("launch browser");
    browser
        .close_before(Instant::now() + Duration::from_secs(15))
        .await
        .expect("bounded close and reap");
}

#[tokio::test]
async fn disposable_context_removes_cookie_and_origin_storage_state() {
    let fixture = Fixture::start();
    let browser = session().await;

    let isolated = timeout(Duration::from_secs(15), browser.new_isolated_context())
        .await
        .expect("create isolated context timed out")
        .expect("isolated context");
    timeout(Duration::from_secs(15), isolated.page().navigate(&fixture.url))
        .await
        .expect("isolated navigation timed out")
        .expect("navigate isolated page");
    assert_eq!(isolated.binding(), BrowserStateBinding::IsolatedBrowserContext);
    assert_eq!(isolated.page().state_binding(), BrowserStateBinding::IsolatedBrowserContext);

    timeout(
        Duration::from_secs(5),
        isolated.page().evaluate_js(
            r#"(() => {
                document.cookie = 'context_cookie=secret; path=/';
                localStorage.setItem('context_local', 'secret');
                sessionStorage.setItem('context_session', 'secret');
                return true;
            })()"#,
        ),
    )
    .await
    .expect("synchronous state seeding timed out")
    .expect("seed synchronous isolated state");

    timeout(
        Duration::from_secs(8),
        isolated.page().evaluate_js(
            r#"Promise.race([
                new Promise((resolve, reject) => {
                    const request = indexedDB.open('context_db', 1);
                    request.onupgradeneeded = () => request.result.createObjectStore('items');
                    request.onsuccess = () => { request.result.close(); resolve(true); };
                    request.onerror = () => reject(request.error);
                }),
                new Promise((_, reject) => setTimeout(() => reject(new Error('indexeddb timeout')), 3000))
            ])"#,
        ),
    )
    .await
    .expect("IndexedDB seeding timed out")
    .expect("seed IndexedDB state");

    let report = timeout(Duration::from_secs(15), isolated.dispose())
        .await
        .expect("context disposal timed out");
    assert_eq!(report.state_binding, BrowserStateBinding::IsolatedBrowserContext);
    assert_eq!(report.disposal_state, ContextDisposalState::Disposed);
    assert!(report.cleanup_complete);

    let clean = timeout(Duration::from_secs(15), browser.new_isolated_context())
        .await
        .expect("create clean context timed out")
        .expect("clean context");
    timeout(Duration::from_secs(15), clean.page().navigate(&fixture.url))
        .await
        .expect("clean navigation timed out")
        .expect("navigate clean page");
    let observed = timeout(
        Duration::from_secs(15),
        clean.page().evaluate_js(
            r#"(async () => ({
                cookie: document.cookie,
                local: localStorage.getItem('context_local'),
                session: sessionStorage.getItem('context_session'),
                databases: (await indexedDB.databases()).map((entry) => entry.name)
            }))()"#,
        ),
    )
    .await
    .expect("clean-state inspection timed out")
    .expect("inspect clean state");
    assert_eq!(observed["cookie"], "");
    assert!(observed["local"].is_null());
    assert!(observed["session"].is_null());
    assert_eq!(observed["databases"].as_array().map(Vec::len), Some(0));

    assert!(
        timeout(Duration::from_secs(15), clean.dispose())
            .await
            .expect("clean context disposal timed out")
            .cleanup_complete
    );
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn dropping_context_disposes_its_page_without_leaking_a_target() {
    let fixture = Fixture::start();
    let browser = session().await;
    let isolated = browser.new_isolated_context().await.expect("isolated context");
    isolated.page().navigate(&fixture.url).await.expect("navigate isolated page");
    let target_id = isolated.page().target_id().to_string();

    drop(isolated);
    for _ in 0..40 {
        let pages = browser.pages().await.expect("list pages");
        if pages.iter().all(|page| page.target_id() != target_id) {
            browser.close().await.expect("close browser");
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("dropped isolated context left its page target alive");
}

#[tokio::test]
async fn cancelling_explicit_context_disposal_does_not_leak_the_context() {
    let browser = session().await;
    let isolated = browser.new_isolated_context().await.expect("isolated context");
    let target_id = isolated.page().target_id();

    let disposal = tokio::spawn(async move { isolated.dispose().await });
    yield_now().await;
    disposal.abort();
    let _ = disposal.await;

    for _ in 0..80 {
        if browser
            .pages()
            .await
            .expect("list pages")
            .iter()
            .all(|page| page.target_id() != target_id)
        {
            browser.close().await.expect("close browser");
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("cancelled isolated-context disposal left its target alive");
}

#[tokio::test]
async fn cancelling_context_creation_after_the_cdp_request_disposes_the_context() {
    let browser = Arc::new(session().await);
    // Chromium's initial about:blank target can arrive asynchronously after
    // launch. Establish a stable baseline so it is not mistaken for a leaked
    // isolated-context target.
    let mut existing_targets = HashSet::new();
    let mut stable_samples = 0_u8;
    for _ in 0..100 {
        let current: HashSet<_> = browser
            .pages()
            .await
            .expect("list initial pages")
            .into_iter()
            .map(|page| page.target_id())
            .collect();
        if current == existing_targets {
            stable_samples = stable_samples.saturating_add(1);
            if stable_samples >= 4 {
                break;
            }
        } else {
            existing_targets = current;
            stable_samples = 0;
        }
        sleep(Duration::from_millis(25)).await;
    }
    let creating_browser = Arc::clone(&browser);

    let creating = tokio::spawn(async move { creating_browser.new_isolated_context().await });
    let mut created_targets = HashSet::new();
    for _ in 0..400 {
        created_targets = browser
            .pages()
            .await
            .expect("list pages during context creation")
            .into_iter()
            .map(|page| page.target_id())
            .filter(|target| !existing_targets.contains(target))
            .collect();
        if !created_targets.is_empty() {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }
    assert!(!created_targets.is_empty(), "context construction never exposed its target");

    creating.abort();
    let _ = creating.await;

    // Cancellation happens only after the new target is observable. The
    // retained construction worker must recover an unreceived context and
    // dispose it rather than leaving that target in Chromium.
    for _ in 0..400 {
        let pages = browser.pages().await.expect("list pages after cancellation");
        if pages.iter().all(|page| !created_targets.contains(&page.target_id())) {
            browser.close().await.expect("close browser");
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("cancelled context creation leaked a target");
}

#[tokio::test]
async fn cancelling_session_close_can_be_retried() {
    let browser = Arc::new(session().await);
    let closing_browser = Arc::clone(&browser);
    let closing = tokio::spawn(async move { closing_browser.close().await });
    yield_now().await;
    closing.abort();
    let _ = closing.await;

    timeout(Duration::from_secs(15), browser.close())
        .await
        .expect("retried close timed out")
        .expect("retried close failed");
    browser.close().await.expect("completed close is idempotent");
}

#[tokio::test]
async fn cancelling_pool_close_after_shutdown_starts_can_be_retried() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 2,
        auto_evict:           false,
    };
    let pool = Arc::new(BrowserPool::new(config, vec![session().await]));
    let held = pool.acquire().await.expect("hold checkout during shutdown");
    let closing_pool = Arc::clone(&pool);
    let closing = tokio::spawn(async move { closing_pool.close().await });

    // acquire() can fail only after close made its one-way transition and
    // closed the semaphore. The retained worker is then deterministically
    // blocked waiting for this checkout, so cancellation cannot race ahead of
    // shutdown-task installation.
    timeout(Duration::from_secs(2), async {
        loop {
            if pool.acquire().await.is_err() {
                break;
            }
            yield_now().await;
        }
    })
    .await
    .expect("pool shutdown did not start");
    assert!(!closing.is_finished(), "close unexpectedly finished with a held checkout");
    closing.abort();
    let _ = closing.await;

    drop(held);
    timeout(Duration::from_secs(15), pool.close())
        .await
        .expect("retried pool close timed out")
        .expect("retried pool close failed");
    pool.close().await.expect("completed pool close is idempotent");
}

#[tokio::test]
async fn pool_close_is_one_way_and_rejects_new_work() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 2,
        auto_evict:           false,
    };
    let pool = BrowserPool::new(config, vec![session().await]);
    pool.close().await.expect("close pool");
    assert!(pool.acquire().await.is_err(), "closed pool accepted acquire");
    assert!(pool.warmup().await.is_err(), "closed pool accepted warmup");
    assert!(pool.evict_idle().await.is_err(), "closed pool accepted eviction");
    pool.close().await.expect("idempotent pool close");
}

#[tokio::test]
async fn pooled_release_clears_document_but_explicitly_retains_shared_origin_state() {
    let fixture = Fixture::start();
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 2,
        auto_evict:           false,
    };
    let pool = BrowserPool::new(config, vec![session().await]);
    let tab = pool.acquire().await.expect("first checkout");
    tab.page.navigate(&fixture.url).await.expect("navigate shared page");
    tab.page
        .evaluate_js(
            "document.cookie='shared_cookie=kept; path=/'; localStorage.setItem('shared','kept')",
        )
        .await
        .expect("seed shared state");

    let report = pool.release_checked(tab).await;
    assert_eq!(report.state_binding, BrowserStateBinding::SharedBrowserProfile);
    assert_eq!(report.strategy, PoolReleaseStrategy::BlankDocumentAndReuseSharedState);
    assert!(report.cleanup_complete);
    assert!(report.document_cleared);
    assert!(report.download_behavior_reset);
    assert!(report.shared_state_retained);

    let tab = pool.acquire().await.expect("second checkout");
    assert_eq!(tab.page.url().await.expect("blank URL").as_deref(), Some("about:blank"));
    tab.page.navigate(&fixture.url).await.expect("return to shared origin");
    let state = tab
        .page
        .evaluate_js("({cookie: document.cookie, local: localStorage.getItem('shared')})")
        .await
        .expect("read retained state");
    assert!(state["cookie"].as_str().is_some_and(|value| value.contains("shared_cookie=kept")));
    assert_eq!(state["local"], "kept");
    pool.release(tab).await;
    pool.close().await.expect("close pool");
}

#[tokio::test]
async fn failed_pool_reset_disposes_tab_and_restores_permit() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 2,
        auto_evict:           false,
    };
    let pool = BrowserPool::new(config, vec![session().await]);
    let tab = pool.acquire().await.expect("first checkout");
    let closed_target = tab.page.target_id();
    tab.page.close().await.expect("force reset failure");

    let report = pool.release_checked(tab).await;
    assert_eq!(report.strategy, PoolReleaseStrategy::DisposeTabAfterResetFailure);
    assert!(!report.cleanup_complete);
    assert!(!report.tab_reused);
    assert!(report.shared_state_retained);
    assert_eq!(pool.available_permits(), 1);

    let replacement = pool.acquire().await.expect("replacement checkout");
    assert_ne!(replacement.page.target_id(), closed_target);
    pool.release(replacement).await;
    pool.close().await.expect("close pool");
}

#[tokio::test]
async fn cancelling_pool_release_still_restores_capacity() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 2,
        auto_evict:           false,
    };
    let pool = Arc::new(BrowserPool::new(config, vec![session().await]));
    let tab = pool.acquire().await.expect("checkout");
    let releasing_pool = Arc::clone(&pool);
    let release = tokio::spawn(async move { releasing_pool.release_checked(tab).await });
    yield_now().await;
    release.abort();
    let _ = release.await;

    for _ in 0..80 {
        if pool.available_permits() == 1 {
            let replacement = pool.acquire().await.expect("replacement checkout");
            pool.release(replacement).await;
            pool.close().await.expect("close pool");
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("cancelled pool release did not restore its permit");
}

#[tokio::test]
async fn dropping_an_unreleased_checkout_restores_capacity() {
    let config = PoolConfig {
        browsers:             1,
        tabs_per_browser:     1,
        tab_max_uses:         50,
        tab_max_idle_secs:    60,
        acquire_timeout_secs: 2,
        auto_evict:           false,
    };
    let pool = BrowserPool::new(config, vec![session().await]);
    let tab = pool.acquire().await.expect("checkout");
    assert_eq!(pool.available_permits(), 0);
    drop(tab);
    assert_eq!(pool.available_permits(), 1);
    pool.close().await.expect("close pool");
}

#[tokio::test]
async fn ordinary_session_pages_are_explicitly_shared_not_isolated() {
    let browser = session().await;
    let page = browser.new_blank_page().await.expect("shared page");
    assert_eq!(page.state_binding(), BrowserStateBinding::SharedBrowserProfile);
    assert!(!page.state_binding().is_isolated());
    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}
