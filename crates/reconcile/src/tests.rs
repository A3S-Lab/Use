use std::io::{Cursor, Write};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::{
    files_sha256, PackageFile, PackageSpec, Payload, ReconcileError, ReconcileStore, SurfaceKind,
    SurfaceSpec,
};

fn file(path: &str, bytes: &str) -> PackageFile {
    PackageFile {
        path: path.to_string(),
        bytes: bytes.as_bytes().to_vec(),
    }
}

fn files_surface(
    kind: SurfaceKind,
    id: &str,
    entry: Option<&str>,
    files: Vec<PackageFile>,
) -> SurfaceSpec {
    let sha256 = files_sha256(&files).unwrap();
    SurfaceSpec {
        kind,
        id: id.to_string(),
        sha256,
        entry: entry.map(str::to_string),
        payload: Payload::Files(files),
    }
}

fn review_surfaces() -> Vec<SurfaceSpec> {
    vec![
        files_surface(
            SurfaceKind::Skill,
            "review",
            None,
            vec![file(
                "skills/review/SKILL.md",
                "---\nname: review\n---\nQuote the document.\n",
            )],
        ),
        files_surface(
            SurfaceKind::Ui,
            "picker",
            Some("ui/picker/index.html"),
            vec![file("ui/picker/index.html", "<p>审查</p>")],
        ),
    ]
}

fn five_surfaces() -> Vec<SurfaceSpec> {
    let mut surfaces = review_surfaces();
    surfaces.push(files_surface(
        SurfaceKind::Okf,
        "notes",
        None,
        vec![file("okf/index.md", "# Review notes\n")],
    ));
    surfaces.push(files_surface(
        SurfaceKind::Mcp,
        "review",
        None,
        vec![file(
            "server.json",
            r#"{"type":"stdio","command":"review-mcp","args":[]}"#,
        )],
    ));
    surfaces.push(files_surface(
        SurfaceKind::Tool,
        "review",
        Some("bin/review"),
        vec![file("bin/review", "#!/bin/sh\necho review\n")],
    ));
    surfaces
}

#[tokio::test]
async fn review_package_publishes_skill_and_ui() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let publication = store
        .apply(&PackageSpec {
            package_id: "a3s/review".to_string(),
            surfaces: review_surfaces(),
        })
        .await
        .unwrap();
    assert_eq!(publication.generation, 1);
    assert_eq!(publication.surfaces.len(), 2);
    let skill = publication
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Skill)
        .unwrap();
    let ui = publication
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Ui)
        .unwrap();
    assert!(skill.directory.join("skills/review/SKILL.md").is_file());
    assert!(ui.entry.as_ref().unwrap().ends_with("ui/picker/index.html"));
    assert!(store.current("a3s/review").await.unwrap().is_some());
}

#[tokio::test]
async fn five_surfaces_publish_as_one_generation() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let spec = PackageSpec {
        package_id: "a3s/review".to_string(),
        surfaces: five_surfaces(),
    };
    let first = store.apply(&spec).await.unwrap();
    let again = store.apply(&spec).await.unwrap();
    assert_eq!(first.generation, 1);
    assert_eq!(again.generation, 1);
    assert_eq!(first.surfaces.len(), 5);
    let mcp = first
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap();
    assert_eq!(mcp.mcp.as_ref().unwrap()["command"], "review-mcp");
    let tool = first
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Tool)
        .unwrap();
    assert!(tool.entry.as_ref().unwrap().ends_with("bin/review"));
}

#[tokio::test]
async fn digest_mismatch_keeps_the_published_generation() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let spec = PackageSpec {
        package_id: "a3s/review".to_string(),
        surfaces: five_surfaces(),
    };
    let published = store.apply(&spec).await.unwrap();
    let skill = published
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Skill)
        .unwrap();
    let original = std::fs::read(skill.directory.join("skills/review/SKILL.md")).unwrap();
    let mut broken = five_surfaces();
    let okf = broken
        .iter_mut()
        .find(|surface| surface.kind == SurfaceKind::Okf)
        .unwrap();
    okf.sha256 =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string();
    let error = store
        .apply(&PackageSpec {
            package_id: "a3s/review".to_string(),
            surfaces: broken,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), ReconcileError::DigestMismatch.code());
    let current = store.current("a3s/review").await.unwrap().unwrap();
    assert_eq!(current.generation, 1);
    assert_eq!(
        std::fs::read(skill.directory.join("skills/review/SKILL.md")).unwrap(),
        original
    );
}

#[tokio::test]
async fn missing_skill_file_does_not_replace_the_generation() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    store
        .apply(&PackageSpec {
            package_id: "a3s/review".to_string(),
            surfaces: review_surfaces(),
        })
        .await
        .unwrap();
    let mut surfaces = review_surfaces();
    let skill = surfaces
        .iter_mut()
        .find(|surface| surface.kind == SurfaceKind::Skill)
        .unwrap();
    let files = vec![file("skills/review/README.md", "no skill file\n")];
    skill.sha256 = files_sha256(&files).unwrap();
    skill.payload = Payload::Files(files);
    let error = store
        .apply(&PackageSpec {
            package_id: "a3s/review".to_string(),
            surfaces,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), "use.reconcile.skill_missing");
    assert_eq!(
        store
            .current("a3s/review")
            .await
            .unwrap()
            .unwrap()
            .generation,
        1
    );
}

#[tokio::test]
async fn zip_link_and_escape_do_not_replace_the_generation() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    store
        .apply(&PackageSpec {
            package_id: "a3s/review".to_string(),
            surfaces: review_surfaces(),
        })
        .await
        .unwrap();

    for bytes in [zip_with_link(), zip_with_escape()] {
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        let error = store
            .apply(&PackageSpec {
                package_id: "a3s/review".to_string(),
                surfaces: vec![SurfaceSpec {
                    kind: SurfaceKind::Skill,
                    id: "review".to_string(),
                    sha256: digest,
                    entry: None,
                    payload: Payload::Zip(bytes),
                }],
            })
            .await
            .unwrap_err();
        assert!(
            error.code() == "use.reconcile.archive_link"
                || error.code() == "use.reconcile.archive_escape"
        );
        assert_eq!(
            store
                .current("a3s/review")
                .await
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        assert_eq!(
            store
                .current("a3s/review")
                .await
                .unwrap()
                .unwrap()
                .surfaces
                .len(),
            2
        );
    }
}

fn zip_with_link() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink(
            "skills/review/SKILL.md",
            "elsewhere",
            SimpleFileOptions::default(),
        )
        .unwrap();
    writer.finish().unwrap().into_inner()
}

#[tokio::test]
async fn withdraw_removes_only_that_package() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    store
        .apply(&PackageSpec {
            package_id: "a3s/review".to_string(),
            surfaces: review_surfaces(),
        })
        .await
        .unwrap();
    store
        .apply(&PackageSpec {
            package_id: "acme/notes".to_string(),
            surfaces: vec![files_surface(
                SurfaceKind::Skill,
                "notes",
                None,
                vec![file("SKILL.md", "# notes\n")],
            )],
        })
        .await
        .unwrap();
    let outside = root.path().join("keep.txt");
    std::fs::write(&outside, "keep").unwrap();

    store.withdraw("a3s/review").await.unwrap();

    assert!(store.current("a3s/review").await.unwrap().is_none());
    let notes = store.current("acme/notes").await.unwrap().unwrap();
    let skill = notes
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Skill)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(skill.directory.join("SKILL.md")).unwrap(),
        "# notes\n"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
    store.withdraw("a3s/review").await.unwrap();
}

fn zip_with_escape() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("../outside.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"nope").unwrap();
    writer.finish().unwrap().into_inner()
}
