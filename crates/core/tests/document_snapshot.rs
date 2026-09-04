//! Deterministic rendered-DOM and accessibility snapshot tests (CAS-311).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use void_crawl_core::{
    AccessibilitySnapshotOptions, BrowserSession, DocumentEpoch, DocumentFrameScope, SnapshotState,
};

fn data_url(html: &str) -> String {
    let encoded = html
        .replace('%', "%25")
        .replace('"', "%22")
        .replace('#', "%23")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace(' ', "%20")
        .replace('\n', "%0A");
    format!("data:text/html,{encoded}")
}

async fn session() -> BrowserSession {
    BrowserSession::builder().headless().no_sandbox().launch().await.expect("launch Chromium")
}

#[tokio::test]
async fn rendered_dom_is_bounded_and_tracks_document_epochs() {
    let browser = session().await;
    let page = browser
        .new_page(&data_url(
            "<main id='value'>source</main><script>document.getElementById('value').textContent='rendered'</script>",
        ))
        .await
        .expect("first page");

    let first = page.rendered_dom_snapshot(1024).await.expect("first DOM snapshot");
    assert_eq!(first.state, SnapshotState::Complete);
    assert!(String::from_utf8_lossy(first.bytes()).contains(">rendered</main>"));
    let DocumentEpoch::Known(first_epoch) = first.scope.epoch else {
        panic!("launched page epoch should be known")
    };

    page.evaluate_js("document.getElementById('value').textContent='spa-update'")
        .await
        .expect("SPA mutation");
    let spa = page.rendered_dom_snapshot(1024).await.expect("SPA DOM snapshot");
    assert_eq!(spa.scope.epoch, DocumentEpoch::Known(first_epoch));
    assert!(String::from_utf8_lossy(spa.bytes()).contains("spa-update"));

    page.navigate(&data_url("<main>second-document</main>")).await.expect("second navigation");
    let second = page.rendered_dom_snapshot(1024).await.expect("second DOM snapshot");
    assert_eq!(second.scope.epoch, DocumentEpoch::Known(first_epoch + 1));

    let truncated = page.rendered_dom_snapshot(8).await.expect("truncated DOM snapshot");
    assert_eq!(truncated.state, SnapshotState::Truncated);
    assert_eq!(truncated.retained_bytes, 8);
    assert!(truncated.complete_bytes.is_some_and(|complete| complete > 8));
    assert_eq!(truncated.bytes().len(), 8);
    assert!(!format!("{truncated:?}").contains("second-document"));

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn accessibility_snapshot_preserves_raw_payload_and_explicit_limits() {
    let browser = session().await;
    let page = browser
        .new_page(&data_url(
            "<main><h1>Account</h1><button>Save</button><input aria-label='Email'></main>",
        ))
        .await
        .expect("AX page");

    let complete = page
        .accessibility_snapshot(AccessibilitySnapshotOptions::default())
        .await
        .expect("complete AX snapshot");
    let repeated = page
        .accessibility_snapshot(AccessibilitySnapshotOptions::default())
        .await
        .expect("repeated AX snapshot");
    assert_eq!(complete.state, SnapshotState::Complete);
    assert!(complete.nodes_observed > 0);
    assert_eq!(complete.nodes_retained, complete.nodes_observed);
    let payload: serde_json::Value =
        serde_json::from_slice(complete.bytes()).expect("raw AX JSON payload");
    assert!(payload.as_array().is_some_and(|nodes| !nodes.is_empty()));
    assert_eq!(complete.bytes(), repeated.bytes(), "unchanged AX serialization must be stable");
    let outline = page.ax_tree_outline(None).await.expect("compact AX projection");
    assert!(!outline.is_empty());
    assert_ne!(complete.bytes(), outline.as_bytes(), "outline is a projection, not raw evidence");

    let node_limited = page
        .accessibility_snapshot(AccessibilitySnapshotOptions {
            max_nodes: 1,
            ..AccessibilitySnapshotOptions::default()
        })
        .await
        .expect("node-limited AX snapshot");
    assert_eq!(node_limited.state, SnapshotState::Truncated);
    assert_eq!(node_limited.nodes_retained, 1);
    assert!(node_limited.nodes_observed > 1);

    let byte_limited = page
        .accessibility_snapshot(AccessibilitySnapshotOptions {
            max_bytes: 4,
            ..AccessibilitySnapshotOptions::default()
        })
        .await
        .expect("byte-limited AX snapshot");
    assert_eq!(byte_limited.state, SnapshotState::Truncated);
    assert!(byte_limited.retained_bytes <= 4);
    assert_eq!(byte_limited.nodes_retained, 0);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(byte_limited.bytes())
            .expect("byte-limited AX remains valid JSON"),
        serde_json::json!([]),
    );
    assert!(!format!("{byte_limited:?}").contains("Email"));

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn frame_accessibility_scope_is_explicit() {
    let browser = session().await;
    let child = data_url("<button aria-label=\"CHILDAX button\">Child</button>");
    let page = browser
        .new_page(&data_url(&format!("<main>parent</main><iframe src='{child}'></iframe>")))
        .await
        .expect("framed page");

    let snapshot = page
        .accessibility_snapshot_in_frame(
            "data:text/html,%3Cbutton%20aria-label",
            AccessibilitySnapshotOptions::default(),
        )
        .await
        .expect("frame AX snapshot");
    assert!(matches!(snapshot.scope.frame, DocumentFrameScope::Frame { .. }));
    assert!(matches!(snapshot.state, SnapshotState::Complete | SnapshotState::Unavailable { .. }));
    if snapshot.state == SnapshotState::Complete {
        assert!(String::from_utf8_lossy(snapshot.bytes()).contains("CHILDAX"));
    }

    page.close().await.expect("close page");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn attached_page_epoch_is_explicitly_unavailable() {
    let owner = session().await;
    let owner_page = owner.new_page(&data_url("<main>owner</main>")).await.expect("owner page");
    let target_id = owner_page.target_id();
    let attached =
        BrowserSession::connect(owner.websocket_url().await).await.expect("attach browser");
    let page = attached.attach_page(&target_id).await.expect("attach exact page");

    let adopted = page.rendered_dom_snapshot(1024).await.expect("adopted snapshot");
    assert_eq!(adopted.scope.epoch, DocumentEpoch::UnavailableForAttachedPage);

    attached.close().await.expect("release attached browser");
    owner.close().await.expect("close owner browser");
}
