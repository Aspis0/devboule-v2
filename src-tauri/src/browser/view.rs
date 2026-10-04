//! The view of a page an agent reads: one line per node, indented by depth,
//! with the role, the name, the states and the ref — and the slice of it one
//! answer carries.
//!
//! How a tree becomes a view is `view_walk`, and how one node becomes a line is
//! `view_line`; this is only what a view is once it exists.

/// The characters a view may carry before it is cut and a cursor offered.
pub const VIEW_BUDGET: usize = 12_000;

/// The roles a person types into, which is what a question about a "field",
/// a "box" or an "input" is asking for.
pub const FIELD_ROLES: [&str; 3] = ["searchbox", "textbox", "combobox"];

/// What the view shows of one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Interactive nodes, plus the headings and landmarks around them.
    Interactive,
    /// Every node the runtime computed a role for.
    Full,
}

/// One line of a view: what it says, and the node it stands for.
#[derive(Debug, Clone)]
pub struct ViewNode {
    /// The `backendDOMNodeId` a ref names. Stable across a re-render, dead on
    /// the next navigation.
    pub backend_id: u64,
    pub role: String,
    pub name: String,
    pub value: String,
    pub description: String,
    pub placeholder: String,
    /// The last piece of text the node's own markup put above it, for `find`.
    pub nearby: String,
    /// The row or list item and the landmark this node is in, as
    /// `in row "…", in navigation "…"`. This is what two controls with the same
    /// name are told apart by.
    pub context: String,
    /// The role of the landmark this node is under, or empty.
    pub landmark: String,
    /// What the markup of a field says about it that the accessible tree does
    /// not: its `type`, `name`, `id` and `placeholder`. Empty until `find`
    /// reads it, and only for the fields a question is about.
    pub hints: String,
    /// How deep a heading is.
    pub level: Option<u8>,
    pub focused: bool,
    pub disabled: bool,
    /// `Some` when the runtime reports a checked state, `None` when the node
    /// has none: a checkbox that did not flip and a control that cannot flip
    /// are different answers.
    pub checked: Option<bool>,
    pub line: String,
}

impl ViewNode {
    pub fn ref_text(&self) -> String {
        format!("e{}", self.backend_id)
    }
}

/// A page's kept nodes, in document order.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub nodes: Vec<ViewNode>,
}

/// One slice of a view: the text, whether more is left, and the ref that
/// continues it.
#[derive(Debug, Clone)]
pub struct Slice {
    pub text: String,
    pub truncated: bool,
    pub cursor: Option<String>,
}

impl View {
    pub fn line_of(&self, backend_id: u64) -> Option<&str> {
        self.nodes
            .iter()
            .find(|node| node.backend_id == backend_id)
            .map(|node| node.line.as_str())
    }

    pub fn focused(&self) -> Option<&ViewNode> {
        self.nodes.iter().find(|node| node.focused)
    }

    /// A dialog the page is showing, if it is showing one: the name of the
    /// dialog, which is what a page says in it.
    pub fn dialog(&self) -> Option<(String, String)> {
        self.nodes
            .iter()
            .find(|node| matches!(node.role.as_str(), "dialog" | "alertdialog"))
            .map(|node| (node.role.clone(), node.name.clone()))
    }

    /// `budget` characters from `from` on. The cursor is the ref of the first
    /// node that did not fit, and it is that node — not an offset — so a
    /// continuation survives a re-render of the page above it.
    pub fn slice(&self, from: usize, budget: usize) -> Slice {
        let mut text = String::new();
        let mut cursor = None;
        for node in self.nodes.iter().skip(from) {
            let line = node.line.clone();
            if !text.is_empty() && text.len() + line.len() + 1 > budget {
                cursor = Some(node.ref_text());
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&line);
        }
        Slice {
            truncated: cursor.is_some(),
            cursor,
            text,
        }
    }
}

/// The ref a node is addressed by, or None. A ref is `e` and the node's
/// `backendDOMNodeId`; nothing else is a ref, so a typo is a missing node and
/// never some other node's.
pub fn parse_ref(text: &str) -> Option<u64> {
    text.strip_prefix('e')?.parse().ok()
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
