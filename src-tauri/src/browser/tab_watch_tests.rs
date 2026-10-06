//! Who a page's own navigations are checked for: a tab an agent opened or
//! drove is the agent's for its life, a tab only the person has used is never
//! checked, and the person's own address bar hands a tab back.

use super::*;

const METADATA: &str = "http://169.254.169.254/latest/meta-data/";
const PRIVATE_PORT: &str = "http://127.0.0.1:5173/";

fn watch_of(drive: &Arc<AgentDrive>) -> TabWatch {
    TabWatch::new(Arc::new(DestinationPolicy::load(None)), Arc::clone(drive))
}

fn url(raw: &str) -> Url {
    Url::parse(raw).expect("a fixture URL")
}

#[test]
fn a_page_an_agent_opened_cannot_script_its_way_to_the_metadata_service() {
    let drive = Arc::new(AgentDrive::opened_by_agent());
    let watch = watch_of(&drive);
    assert!(!drive.in_flight(), "no agent command is running");
    assert!(watch.blocked(&url(METADATA)).is_some());
    assert!(
        drive.take_refusal().is_some(),
        "the refusal waits for the agent's next answer"
    );
}

#[test]
fn a_page_an_agent_navigated_stays_checked_after_the_command_returns() {
    let drive = Arc::new(AgentDrive::default());
    let watch = watch_of(&drive);
    {
        let _command = drive.begin();
        assert!(watch.blocked(&url(METADATA)).is_some());
    }
    assert!(!drive.in_flight());
    assert!(watch.blocked(&url(METADATA)).is_some());
    assert!(
        watch.blocked(&url("http://192.168.1.1/admin")).is_some(),
        "a link the person clicks in that tab is a navigation of that tab"
    );
}

#[test]
fn the_persons_own_address_bar_hands_the_tab_back() {
    let drive = Arc::new(AgentDrive::opened_by_agent());
    let watch = watch_of(&drive);
    drive.clear_taint();
    assert!(watch.blocked(&url(PRIVATE_PORT)).is_none());
    assert!(
        watch.blocked(&url("http://10.0.0.1/")).is_none(),
        "the tab is the person's until an agent touches it again"
    );
    let _command = drive.begin();
    assert!(watch.blocked(&url(PRIVATE_PORT)).is_some());
}

#[test]
fn a_tab_only_the_person_used_is_never_checked() {
    let drive = Arc::new(AgentDrive::default());
    let watch = watch_of(&drive);
    for raw in [PRIVATE_PORT, METADATA, "http://192.168.1.1/"] {
        assert!(watch.blocked(&url(raw)).is_none(), "{raw}");
    }
    assert!(drive.take_refusal().is_none());
}

#[test]
fn a_frame_of_an_agent_tab_is_checked_like_a_navigation() {
    let drive = Arc::new(AgentDrive::opened_by_agent());
    let watch = watch_of(&drive);
    assert!(watch.blocked_frame(METADATA).is_some());
    assert!(watch.blocked_frame("http://[::1]:8080/").is_some());
    assert!(
        watch.blocked_frame("not a url").is_some(),
        "an address nothing can read is not vouched for"
    );
}

#[test]
fn frames_that_carry_their_own_content_are_not_destinations() {
    let drive = Arc::new(AgentDrive::opened_by_agent());
    let watch = watch_of(&drive);
    for raw in ["about:blank", "about:srcdoc", "data:text/html,hi"] {
        assert!(watch.blocked_frame(raw).is_none(), "{raw}");
    }
}

#[test]
fn a_frame_of_the_persons_tab_is_never_checked() {
    let drive = Arc::new(AgentDrive::default());
    let watch = watch_of(&drive);
    assert!(watch.blocked_frame(METADATA).is_none());
    assert!(watch.blocked_frame("not a url").is_none());
}
