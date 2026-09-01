use std::fs;
use std::path::{Path, PathBuf};

use oracle_core::{CkgEdgeRow, CkgStore, OracleDataPaths};
use polis_backend::{build_city, CityBuildError};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("city-parity")
}

#[test]
fn builder_matches_fixture_ids_lines_districts_and_import_weights() {
    let city = build_city(&fixture_root()).expect("fixture city should build");
    let files = city["files"].as_array().expect("files array");
    assert_eq!(files.len(), 3);

    let rows: Vec<(String, String, u64, String)> = files
        .iter()
        .map(|file| {
            (
                file["id"].as_str().unwrap().to_string(),
                file["path"].as_str().unwrap().to_string(),
                file["lines"].as_u64().unwrap(),
                file["district"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "src/empty.ts".into(),
                "src/empty.ts".into(),
                0,
                "src".into()
            ),
            (
                "src/helper.ts".into(),
                "src/helper.ts".into(),
                1,
                "src".into()
            ),
            ("src/main.ts".into(), "src/main.ts".into(), 5, "src".into()),
        ]
    );

    let imports = city["imports"].as_array().unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["from"], "src/main.ts");
    assert_eq!(imports[0]["to"], "src/helper.ts");
    assert_eq!(imports[0]["weight"], 3);
    assert_eq!(city["agents"], serde_json::json!([]));
    assert_eq!(city["findings"], serde_json::json!([]));
    assert_eq!(city["dataSource"], "host");
}

#[test]
fn line_counts_match_crlf_cr_lf_and_trailing_newline_semantics() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("crlf.ts"), b"one\r\ntwo\r\n").unwrap();
    fs::write(temp.path().join("cr.ts"), b"one\rtwo\r").unwrap();
    fs::write(temp.path().join("no-trailing.ts"), b"one\ntwo").unwrap();
    fs::write(temp.path().join("empty.ts"), b"").unwrap();

    let city = build_city(temp.path()).expect("line-count fixture should build");
    let lines = city["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_string(),
                file["lines"].as_u64().unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(lines["crlf.ts"], 2);
    assert_eq!(lines["cr.ts"], 2);
    assert_eq!(lines["no-trailing.ts"], 2);
    assert_eq!(lines["empty.ts"], 0);
}

#[test]
fn undecodable_file_is_counted_from_bytes_and_does_not_kill_the_city() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("good.ts"), b"one\ntwo\n").unwrap();
    fs::write(
        temp.path().join("undecodable.ts"),
        [b'a', b'\n', 0xff, b'\r'],
    )
    .unwrap();

    let city = build_city(temp.path()).expect("an undecodable file must not abort the city");
    let files = city["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "good.ts"));
    assert_eq!(
        files
            .iter()
            .find(|file| file["path"] == "undecodable.ts")
            .unwrap()["lines"],
        2
    );
    assert!(city.get("skippedFiles").is_none());
}

#[test]
fn unreadable_root_is_a_named_refusal_not_an_empty_city() {
    let temp = tempfile::tempdir().unwrap();
    let not_a_directory = temp.path().join("root-file");
    fs::write(&not_a_directory, "not a directory").unwrap();

    let error = build_city(&not_a_directory).expect_err("read_dir must refuse this root");
    assert!(matches!(error, CityBuildError::UnreadableRoot { .. }));
    assert!(error.to_string().contains("city root unreadable"));
}

#[test]
fn ckg_edges_are_read_only_and_collapsed_by_resolved_file_pair() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/a.ts"), "// regex must not be used\n").unwrap();
    fs::write(root.join("src/b.ts"), "export const b = 1;\n").unwrap();

    let ckg = CkgStore::new(&OracleDataPaths::from_root(root).ckg).unwrap();
    ckg.replace_all(
        &[],
        &[
            CkgEdgeRow {
                src: "src/a.ts#one".into(),
                dst: "src/b.ts".into(),
                kind: "IMPORT".into(),
                src_file: "src/a.ts".into(),
            },
            CkgEdgeRow {
                src: "src/a.ts#two".into(),
                dst: "src/b.ts".into(),
                kind: "IMPORT".into(),
                src_file: "src/a.ts".into(),
            },
        ],
    )
    .unwrap();

    let city = build_city(root).expect("city should use the CKG");
    assert_eq!(
        city["imports"],
        serde_json::json!([{
            "from": "src/a.ts",
            "to": "src/b.ts",
            "weight": 2,
        }])
    );
}

#[test]
fn absent_ckg_falls_back_to_extractor_compatible_regex_edges() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("src/a.ts"),
        "import x from \"./b\";\nimport y from \"./b\";\nimport packageName from \"package\";\n",
    )
    .unwrap();
    fs::write(root.join("src/b.ts"), "export const b = 1;\n").unwrap();

    let city = build_city(root).expect("city should fall back without a CKG");
    assert_eq!(
        city["imports"],
        serde_json::json!([{
            "from": "src/a.ts",
            "to": "src/b.ts",
            "weight": 2,
        }])
    );
}

#[test]
fn city_ignores_an_oracle_dir_override_from_another_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let foreign = temp.path().join("foreign");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(foreign.join("oracle-data")).unwrap();
    fs::write(root.join("src/a.ts"), "export const a = 1;\n").unwrap();
    fs::write(root.join("src/b.ts"), "export const b = 1;\n").unwrap();

    let foreign_paths = OracleDataPaths::from_root_without_env(&foreign);
    let ckg = CkgStore::new(&foreign_paths.ckg).unwrap();
    ckg.replace_all(
        &[],
        &[CkgEdgeRow {
            src: "src/a.ts#foreign".into(),
            dst: "src/b.ts".into(),
            kind: "IMPORT".into(),
            src_file: "src/a.ts".into(),
        }],
    )
    .unwrap();

    let previous = std::env::var_os("ORACLE_DIR");
    std::env::set_var("ORACLE_DIR", foreign_paths.root);
    let city = build_city(&root).expect("city should use only the granted root");
    match previous {
        Some(value) => std::env::set_var("ORACLE_DIR", value),
        None => std::env::remove_var("ORACLE_DIR"),
    }
    assert_eq!(city["imports"], serde_json::json!([]));
}

#[test]
fn measure_devboule_v2_runtime_city_walk() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("devboule-v2 root");
    let started = std::time::Instant::now();
    let city = build_city(repo).expect("devboule-v2 city should build");
    let elapsed = started.elapsed();
    println!(
        "devboule-v2 city walk: {} ms, {} files, {} imports, {} JSON bytes",
        elapsed.as_millis(),
        city["files"].as_array().map_or(0, Vec::len),
        city["imports"].as_array().map_or(0, Vec::len),
        serde_json::to_vec(&city).expect("city JSON").len(),
    );
}

#[test]
fn city_sanity_caps_are_bounded_and_non_text_files_are_not_city_files() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..(polis_backend::MAX_CITY_FILES + 5) {
        fs::write(
            temp.path().join(format!("file-{index:04}.ts")),
            "export const value = 1;\n",
        )
        .unwrap();
    }
    fs::write(temp.path().join("image.bin"), [0u8, 1, 2, 3]).unwrap();
    let city = build_city(temp.path()).expect("bounded city should build");
    assert_eq!(
        city["files"].as_array().unwrap().len(),
        polis_backend::MAX_CITY_FILES
    );
    assert_eq!(city["truncatedFiles"], 1);
    assert!(city["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| file["path"] != "image.bin"));
}

#[test]
fn city_uses_the_two_megabyte_file_cap() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("medium.ts"), "x".repeat(1_500_000)).unwrap();
    fs::write(
        temp.path().join("large.ts"),
        "x".repeat((polis_backend::MAX_CITY_FILE_BYTES + 1) as usize),
    )
    .unwrap();

    let city = build_city(temp.path()).expect("file-cap fixture should build");
    let files = city["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "medium.ts"));
    assert!(files.iter().all(|file| file["path"] != "large.ts"));
    assert_eq!(city["skippedFiles"], 1);
}

#[test]
fn city_serialization_is_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/a.ts"), "import b from './b';\n").unwrap();
    fs::write(temp.path().join("src/b.ts"), "export const b = 1;\n").unwrap();

    let first = serde_json::to_vec(&build_city(temp.path()).unwrap()).unwrap();
    let second = serde_json::to_vec(&build_city(temp.path()).unwrap()).unwrap();
    assert_eq!(first, second);
}
