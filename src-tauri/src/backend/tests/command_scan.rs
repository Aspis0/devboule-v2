//! Read the sources at test time: walk every `.rs` under `src/`, parse it,
//! and report the `#[tauri::command]`s it declares together with the waits it
//! sends through the blocking helper.
//!
//! The pins lean on two properties bought here. A file that stops parsing
//! fails the test with its name instead of vanishing from the scan, and a
//! command written inside a comment is not a command — `syn` reads code, not
//! text. A list of file names would go stale the day a file is added; this
//! walk sees the new file by itself.

use std::path::{Path, PathBuf};

use syn::visit::Visit;

pub(super) struct CommandDecl {
    pub(super) file: PathBuf,
    pub(super) name: String,
    pub(super) is_public: bool,
    pub(super) is_async: bool,
    pub(super) takes_daemon_bridge: bool,
}

pub(super) struct SourceScan {
    pub(super) commands: Vec<CommandDecl>,
    pub(super) helper_calls: usize,
}

struct FileScan {
    file: PathBuf,
    commands: Vec<CommandDecl>,
    helper_calls: usize,
}

impl FileScan {
    fn record(
        &mut self,
        attrs: &[syn::Attribute],
        sig: &syn::Signature,
        visibility: &syn::Visibility,
    ) {
        if !attrs.iter().any(is_command_attr) {
            return;
        }
        self.commands.push(CommandDecl {
            file: self.file.clone(),
            name: sig.ident.to_string(),
            is_public: matches!(visibility, syn::Visibility::Public(_)),
            is_async: sig.asyncness.is_some(),
            takes_daemon_bridge: sig.inputs.iter().any(|argument| match argument {
                syn::FnArg::Typed(typed) => is_daemon_bridge_state(&typed.ty),
                syn::FnArg::Receiver(_) => false,
            }),
        });
    }
}

/// `#[tauri::command]` as every file spells it today, and `#[command]`
/// should someone import the macro — the segments are the whole spelling
/// check, the attributes and their arguments are ignored.
fn is_command_attr(attr: &syn::Attribute) -> bool {
    let segments = &attr.path().segments;
    match segments.len() {
        1 => segments[0].ident == "command",
        2 => segments[0].ident == "tauri" && segments[1].ident == "command",
        _ => false,
    }
}

/// `State<'_, DaemonBridge>`: the pin keys on what the command holds, so an
/// elided lifetime or a qualified path to the same type both count.
fn is_daemon_bridge_state(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "State" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    arguments.args.iter().any(|argument| match argument {
        syn::GenericArgument::Type(syn::Type::Path(inner_path)) => inner_path
            .path
            .segments
            .last()
            .is_some_and(|inner| inner.ident == "DaemonBridge"),
        _ => false,
    })
}

impl<'ast> Visit<'ast> for FileScan {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.record(&node.attrs, &node.sig, &node.vis);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.record(&node.attrs, &node.sig, &node.vis);
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*node.func {
            if path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "off_main_thread")
            {
                self.helper_calls += 1;
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
}

/// Every `.rs` file under the package's `src/`, sorted, so a failure names
/// files in a stable order.
pub(super) fn scan() -> SourceScan {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_sources(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no sources found under {} — the pins would pass on nothing",
        root.display()
    );

    let mut scan = SourceScan {
        commands: Vec::new(),
        helper_calls: 0,
    };
    for file in files {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
        let parsed = syn::parse_file(&text)
            .unwrap_or_else(|error| panic!("{} no longer parses: {error}", file.display()));
        let mut one = FileScan {
            file: file
                .strip_prefix(&root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| file.clone()),
            commands: Vec::new(),
            helper_calls: 0,
        };
        one.visit_file(&parsed);
        scan.commands.extend(one.commands);
        scan.helper_calls += one.helper_calls;
    }
    scan
}

fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|error| panic!("cannot read an entry of {}: {error}", dir.display()));
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
