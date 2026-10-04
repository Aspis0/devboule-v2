use super::*;
use crate::browser::ax::AxTree;
use crate::browser::test_pages::{bare_fields, encyclopedia, front_page, news};
use crate::browser::test_support::{ax_fixture, ax_property, buttons};
use crate::browser::view::Mode;
use crate::browser::view_walk::compact;

fn view_of(checked: &str) -> View {
    let mut fixture = ax_fixture();
    for node in fixture["nodes"]
        .as_array_mut()
        .expect("the fixture has nodes")
    {
        if node["role"]["value"] == "checkbox" {
            node["properties"] = serde_json::json!([ax_property(
                "checked",
                "tristate",
                serde_json::json!(checked)
            )]);
        }
    }
    let tree: AxTree = serde_json::from_value(fixture).expect("the fixture parses");
    compact(&tree, Mode::Interactive)
}

#[test]
fn the_name_is_what_a_query_is_looking_for() {
    let view = view_of("false");

    let hits = find(&view, "remember");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e14");
    assert_eq!(hits[0].role, "checkbox");
    assert_eq!(hits[0].name, "Remember me");
    // The context is what tells this control from another of the same name.
    assert_eq!(hits[0].context, r#"in form "Sign in""#);
}

#[test]
fn a_query_is_not_case_sensitive_and_surrounding_spaces_do_not_matter() {
    let view = view_of("false");

    assert_eq!(find(&view, "  REMEMBER ME  ")[0].reference, "e14");
}

#[test]
fn a_role_is_a_way_to_ask_and_the_whole_of_that_role_answers() {
    let view = view_of("false");

    let hits = find(&view, "textbox");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e13", "the field is the only textbox");
}

#[test]
fn a_control_with_no_name_is_found_by_the_words_around_it() {
    let view = view_of("false");

    // The icon button has no name of its own; the paragraph above it is what
    // answers for it.
    let hits = find(&view, "forgot your password");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e17");
    assert_eq!(hits[0].name, "", "and it is the control with no name");

    // The words above a checkbox answer for the checkbox, and a control that
    // is one node further down shares those words: the query says a place, not
    // a line, and the first in document order is what it means.
    let above = find(&view, "keep me signed in");
    assert_eq!(above[0].reference, "e14");
}

#[test]
fn a_field_is_found_by_the_value_it_holds() {
    let view = view_of("false");

    let hits = find(&view, "person@example.test");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, "e13");
}

#[test]
fn a_query_nothing_says_matches_nothing_rather_than_everything() {
    let view = view_of("true");

    assert!(find(&view, "quantum entanglement").is_empty());
    assert!(
        find(&view, "   ").is_empty(),
        "an empty query is not a match"
    );
}

#[test]
fn the_best_match_comes_first_and_document_order_breaks_a_tie() {
    let view = view_of("false");

    // Three nodes are named "Sign in" and all three are exact matches: the same
    // score, so the one earlier in the document comes first.
    let hits = find(&view, "sign in");
    let refs: Vec<&str> = hits.iter().map(|hit| hit.reference.as_str()).collect();
    assert_eq!(refs[..3], ["e11", "e19", "e15"]);
    assert_eq!(
        refs.len(),
        6,
        "and every control in the form is a weaker match, on the name of the          form they are in"
    );
}

#[test]
fn a_query_answers_at_most_twenty_matches() {
    let view = compact(&buttons(50, "Save"), Mode::Interactive);

    let hits = find(&view, "save");
    assert_eq!(hits.len(), MAX_MATCHES);
    // Every one of them is the same score, so the answer is the first twenty
    // in the order the page has them.
    assert_eq!(hits[0].reference, "e1");
    assert_eq!(hits[MAX_MATCHES - 1].reference, "e20");
}

fn find(view: &View, query: &str) -> Vec<Match> {
    rank(view, &Query::parse(query))
}

fn refs(hits: &[Match]) -> Vec<&str> {
    hits.iter().map(|hit| hit.reference.as_str()).collect()
}

fn interactive(tree: &AxTree) -> View {
    compact(tree, Mode::Interactive)
}

#[test]
fn a_search_box_is_the_field_in_the_search_landmark_whatever_it_is_called() {
    let view = interactive(&encyclopedia());

    let hits = find(&view, "search box");
    // The "Search" button answers to the word and is not a box: it is there,
    // behind the field.
    assert_eq!(refs(&hits), ["e905", "e906"]);
    assert_eq!(hits[0].role, "combobox");
    assert_eq!(hits[0].name, "", "and the page never named it");
    assert_eq!(hits[0].context, "in search");
}

#[test]
fn every_way_to_ask_for_a_field_finds_the_same_one() {
    let view = interactive(&encyclopedia());

    for asked in [
        "search bar",
        "search field",
        "search input",
        "the search textbox",
        "SEARCH BOX",
    ] {
        assert_eq!(refs(&find(&view, asked))[0], "e905", "{asked}");
    }
}

#[test]
fn a_field_with_nothing_around_it_is_found_by_what_its_markup_says() {
    let mut view = interactive(&bare_fields());
    assert!(
        find(&view, "search box").is_empty(),
        "an unnamed field in no landmark says nothing about search"
    );

    // What `find` reads off the page for a field: its type, name and id.
    view.nodes[0].hints = "search q searchInput".to_owned();

    let hits = find(&view, "search box");
    assert_eq!(refs(&hits), ["e1"]);
}

#[test]
fn a_kind_of_control_is_answered_by_that_kind_alone() {
    let view = interactive(&encyclopedia());

    assert_eq!(refs(&find(&view, "button")), ["e906"]);
    assert_eq!(
        refs(&find(&view, "links")),
        ["e903", "e908", "e909", "e910"],
        "every link and no button or field among them: the header's and the          navigation's first, the footer's after, each in document order"
    );
    assert_eq!(refs(&find(&view, "input")), ["e905"]);
}

#[test]
fn the_kind_and_the_name_narrow_together() {
    let view = interactive(&encyclopedia());

    assert_eq!(refs(&find(&view, "search button"))[0], "e906");
    assert_eq!(refs(&find(&view, "history link")), ["e908"]);
}

#[test]
fn a_page_that_styled_a_link_as_a_button_is_still_found_by_its_name() {
    let view = interactive(&encyclopedia());

    // There is no button called this, so the whole page is read.
    assert_eq!(refs(&find(&view, "main page button")), ["e903"]);
}

#[test]
fn a_control_in_the_navigation_comes_before_the_same_name_in_the_footer() {
    let view = interactive(&encyclopedia());

    // The footer's "Help" is first in the document, and the navigation's is the
    // one a person means when they say nothing about where.
    let hits = find(&view, "help");
    assert_eq!(refs(&hits), ["e909", "e910"]);
    assert_eq!(hits[0].context, r#"in navigation "Contents""#);
    assert_eq!(hits[1].context, "in contentinfo");
}

#[test]
fn naming_the_footer_puts_the_footer_first() {
    let view = interactive(&encyclopedia());

    // The navigation's is still an answer, behind it.
    assert_eq!(refs(&find(&view, "footer help")), ["e910", "e909"]);
    assert_eq!(refs(&find(&view, "footer help link")), ["e910", "e909"]);
}

#[test]
fn a_result_says_which_row_it_is_in_so_three_comments_links_are_three_answers() {
    let view = interactive(&front_page());

    // The title links share the row's words, so they follow as weaker answers.
    let hits = find(&view, "comments");
    assert_eq!(refs(&hits)[..3], ["e917", "e927", "e937"]);
    let contexts: Vec<&str> = hits[..3].iter().map(|hit| hit.context.as_str()).collect();
    assert_eq!(
        contexts,
        [
            r#"in row "1. First story 12 comments""#,
            r#"in row "2. Second story 3 comments""#,
            r#"in row "3. Third story 7 comments""#,
        ]
    );
}

#[test]
fn a_link_in_the_top_bar_names_the_bar() {
    let view = interactive(&front_page());

    let hits = find(&view, "login");
    assert_eq!(refs(&hits), ["e904"]);
    assert_eq!(hits[0].context, r#"in navigation "top bar""#);
}

#[test]
fn a_place_the_page_does_not_have_does_not_drop_the_link_the_name_finds() {
    // The top bar of this page is a table row, not a navigation landmark, so
    // "in the top bar" matches no place at all.
    let view = interactive(&news(30));

    // The other links of the bar are there too, behind it: they say "new"
    // around them and are not called it.
    let hits = find(&view, "new link in the top bar");
    assert_eq!(refs(&hits)[0], "e904");

    assert_eq!(
        refs(&find(&view, "past link in the footer"))[0],
        "e905",
        "and a footer the page does not have is not a reason to find nothing"
    );
}

#[test]
fn the_top_of_the_page_comes_before_thirty_rows_that_repeat_the_name() {
    let view = interactive(&news(30));

    let hits = find(&view, "past");

    assert_eq!(
        hits.len(),
        MAX_MATCHES,
        "thirty-one links answer, twenty are kept"
    );
    assert_eq!(hits[0].reference, "e905", "the top bar's, not a row's");
    assert_eq!(
        refs(&hits)[1],
        "e1018",
        "and the rows follow in document order"
    );
}

#[test]
fn a_repeated_name_ranks_by_document_order_even_when_the_rows_say_it_too() {
    // Every row's own words contain "past", which must not lift a row's link over
    // the one that came first.
    let view = interactive(&news(30));
    let row = view
        .nodes
        .iter()
        .find(|node| node.backend_id == 1_018)
        .expect("the first row's link is a line");
    assert!(row.context.contains("past"), "{}", row.context);

    assert_eq!(find(&view, "past")[0].reference, "e905");
}

#[test]
fn a_nameless_match_says_what_it_is_and_a_named_one_does_not() {
    let mut view = interactive(&bare_fields());
    view.nodes[0].hints = "type=search name=q".to_owned();
    view.nodes[0].nearby = "Search the encyclopedia".to_owned();

    let bare = find(&view, "search box");
    assert_eq!(bare[0].reference, "e1");
    assert_eq!(
        bare[0].detail,
        r#"type=search name=q, near "Search the encyclopedia""#
    );

    let named = find(&view, "email");
    assert_eq!(named[0].reference, "e2");
    assert_eq!(named[0].detail, "");
}
