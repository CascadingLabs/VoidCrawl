use std::time::{Duration, Instant};

use chromiumoxide::{
    CdpMode,
    cmd::CommandChain,
    handler::{frame::FrameManager, network::NetworkManager},
};
use futures::task::Poll;

fn collect_methods(mut cmds: CommandChain) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    loop {
        match cmds.poll(Instant::now()) {
            Poll::Ready(Some(Ok((method, _params)))) => {
                let method_name = method.as_ref().to_string();
                assert!(cmds.received_response(&method_name));
                out.push(method_name);
            }
            Poll::Ready(None) => return Ok(out),
            other => return Err(format!("unexpected command poll: {other:?}")),
        }
    }
}

#[test]
fn minimal_frame_init_skips_runtime_until_lazy_frame_escalation() -> Result<(), String> {
    assert_eq!(
        collect_methods(FrameManager::init_commands(Duration::from_secs(1), CdpMode::Minimal,))?,
        vec!["Page.enable", "Page.getFrameTree", "Page.setLifecycleEventsEnabled",],
    );

    assert_eq!(
        collect_methods(FrameManager::init_commands(Duration::from_secs(1), CdpMode::Normal,))?,
        vec![
            "Page.enable",
            "Page.getFrameTree",
            "Page.setLifecycleEventsEnabled",
            "Runtime.enable",
        ],
    );
    Ok(())
}

#[test]
fn minimal_network_init_preserves_ignore_https_without_network_enable() -> Result<(), String> {
    let timeout = Duration::from_secs(1);

    assert_eq!(
        collect_methods(NetworkManager::new(true, timeout).init_commands(CdpMode::Minimal))?,
        vec!["Security.setIgnoreCertificateErrors"],
    );

    assert!(
        collect_methods(NetworkManager::new(false, timeout).init_commands(CdpMode::Minimal))?
            .is_empty()
    );

    assert_eq!(
        collect_methods(NetworkManager::new(true, timeout).init_commands(CdpMode::Normal))?,
        vec!["Network.enable", "Security.setIgnoreCertificateErrors"],
    );
    Ok(())
}
