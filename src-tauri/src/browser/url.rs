//! The gate every URL a browser tab shows must pass, asked twice: once when
//! the address bar submits a string, and once inside `on_navigation`, where a
//! redirect, a `location.assign` or a scripted frame arrives with no user
//! behind it. A check that only the address bar runs is a check a page walks
//! around, so both answers come from here.

use tauri::Url;

/// Why a URL was refused. Carried to the frontend as the inline error line,
/// so the words name the scheme rather than saying "invalid".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Text that is not a URL at all, or a scheme with no authority.
    Unusable(String),
    /// A scheme this tab will never load: `file:`, `javascript:`, a custom
    /// scheme, or anything else that is not http(s).
    NotWeb(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unusable(_) => write!(f, "That is not a web address."),
            Refusal::NotWeb(scheme) => write!(f, "`{scheme}:` addresses cannot be opened here."),
        }
    }
}

/// The one gate. http and https only: `file:` would hand a remote page the
/// disk, `javascript:` and `data:` run in this app's process context, and a
/// custom scheme would let a page call into whatever registered it.
///
/// The scheme is read off the parsed URL, never off the text, so `HTTPS:` and
/// a newline before `javascript:` land on the same answer as their plain
/// spellings.
pub fn gate(candidate: &Url) -> Result<(), Refusal> {
    if !matches!(candidate.scheme(), "http" | "https") {
        return Err(Refusal::NotWeb(candidate.scheme().to_owned()));
    }
    Ok(())
}

/// Gate a URL the frontend is about to send. The frontend already normalises
/// and resolves text to a full URL (see `src/features/workspace/browserUrl.ts`),
/// so this only re-reads the parse and refuses; it never rewrites, because a
/// URL that needed rewriting has already been through a second policy.
///
/// The refusal comes back as the sentence the chrome shows, so a command has
/// nothing left to phrase.
pub fn accept(raw: &str) -> Result<Url, String> {
    let parsed =
        Url::parse(raw.trim()).map_err(|_| Refusal::Unusable(raw.trim().to_owned()).to_string())?;
    gate(&parsed).map_err(|refusal| refusal.to_string())?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert!(accept("https://example.com/page?q=1").is_ok());
        // This gate is the scheme only: whether the destination is one agent
        // browsing may reach is `destination::DestinationPolicy`'s answer.
        assert!(accept("http://example.com:8080/").is_ok());
    }

    #[test]
    fn refuses_file_javascript_data_and_custom_schemes() {
        for raw in [
            "file:///C:/Windows/System32/drivers/etc/hosts",
            "javascript:window.__TAURI__",
            "data:text/html,<script>1</script>",
            "tauri://localhost",
            "ipc://localhost",
            "about:blank",
            "ftp://example.com",
        ] {
            let scheme = raw.split(':').next().unwrap_or_default().to_lowercase();
            assert_eq!(
                accept(raw).expect_err("must refuse"),
                format!("`{scheme}:` addresses cannot be opened here."),
                "{raw} did not name the scheme it refused"
            );
        }
    }

    #[test]
    fn refuses_text_that_is_not_a_url() {
        // The parser is the authority on whether a spelling has an address:
        // `http://` has none and is refused as text, while `http:/example.com`
        // has an address the parser supplies and is loaded as one.
        const NOT_A_URL: &str = "That is not a web address.";
        assert_eq!(accept("not a url").expect_err("must refuse"), NOT_A_URL);
        assert_eq!(accept("").expect_err("must refuse"), NOT_A_URL);
        assert_eq!(accept("   ").expect_err("must refuse"), NOT_A_URL);
        assert_eq!(accept("https://").expect_err("must refuse"), NOT_A_URL);
        assert_eq!(accept("http://").expect_err("must refuse"), NOT_A_URL);
    }

    #[test]
    fn reads_a_single_slash_spelling_as_the_address_it_normalises_to() {
        assert_eq!(
            accept("http:/example.com").expect("must load").to_string(),
            "http://example.com/"
        );
    }

    #[test]
    fn reads_the_scheme_off_the_parse_not_the_text() {
        // A page that navigates to a URL spelled in capitals, or built by
        // concatenating a newline in front of the scheme, is refused exactly like
        // the plain spelling.
        let shouted = Url::parse("JAVASCRIPT:alert(1)").expect("parses");
        assert!(matches!(gate(&shouted), Err(Refusal::NotWeb(_))));
        let upper = Url::parse("HTTPS://example.com").expect("parses");
        assert!(gate(&upper).is_ok());
    }

    #[test]
    fn the_refusal_names_the_scheme_it_refused() {
        assert_eq!(
            Refusal::NotWeb("file".to_owned()).to_string(),
            "`file:` addresses cannot be opened here."
        );
        assert_eq!(
            accept("file:///etc/passwd").expect_err("must refuse"),
            "`file:` addresses cannot be opened here."
        );
    }
}
