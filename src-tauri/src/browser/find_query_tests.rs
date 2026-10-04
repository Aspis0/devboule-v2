use super::*;

#[test]
fn a_question_that_names_no_kind_of_control_is_kept_whole() {
    let query = Query::parse("  Sign In  ");

    assert!(query.roles.is_empty());
    assert_eq!(query.needle, "sign in");
    assert_eq!(query.words, vec!["sign", "in"]);
}

#[test]
fn the_ways_to_ask_for_a_field_are_one_ask() {
    for asked in [
        "search box",
        "search bar",
        "search field",
        "search input",
        "the search textbox",
    ] {
        let query = Query::parse(asked);
        assert!(query.asks_for_fields(), "{asked}");
        assert_eq!(query.roles, FIELD_ROLES.to_vec(), "{asked}");
        assert!(query.words.contains(&"search".to_owned()), "{asked}");
    }
}

#[test]
fn a_role_word_is_taken_out_of_what_the_control_is_called() {
    let query = Query::parse("Sign in button");

    assert_eq!(query.roles, vec!["button"]);
    assert_eq!(query.needle, "sign in");
    assert!(!query.asks_for_fields());
}

#[test]
fn an_article_is_not_part_of_what_a_control_is_called() {
    let query = Query::parse("the search box");
    assert_eq!(query.words, vec!["search"]);

    // A question that is nothing but an article still asks for something.
    assert_eq!(Query::parse("a").needle, "a");
}

#[test]
fn a_question_made_only_of_a_kind_has_nothing_left_to_match_by_name() {
    let query = Query::parse("links");

    assert_eq!(query.roles, vec!["link"], "the plural names the same kind");
    assert!(query.needle.is_empty());
    assert!(query.words.is_empty());
}

#[test]
fn a_word_that_only_contains_a_role_word_is_not_one() {
    for asked in ["table", "tabular data", "boxing", "linked list"] {
        assert!(Query::parse(asked).roles.is_empty(), "{asked}");
    }
}

#[test]
fn a_word_for_a_part_of_the_page_is_a_landmark_to_look_in_and_not_part_of_the_name() {
    let query = Query::parse("footer help link");

    assert_eq!(query.places, vec!["contentinfo"]);
    assert_eq!(query.roles, vec!["link"]);
    assert_eq!(query.needle, "help");

    assert_eq!(Query::parse("nav login").places, vec!["navigation"]);
    assert_eq!(Query::parse("the header search box").places, vec!["banner"]);
}

#[test]
fn a_question_made_only_of_a_place_is_about_a_control_called_that() {
    let query = Query::parse("Footer");

    assert!(query.places.is_empty());
    assert_eq!(query.needle, "footer");
}

#[test]
fn a_question_says_where_to_look_only_in_the_words_for_a_part_of_the_page() {
    assert!(Query::parse("login in the header").names_a_place());
    assert!(Query::parse("footer links").names_a_place());
    assert!(Query::parse("row of results").names_a_place());
    assert!(!Query::parse("login").names_a_place());
    assert!(!Query::parse("search box").names_a_place());
}
