//! Photo engine 1: high-bit-depth RGB storage and the floating-point working
//! representation of `docs/photography-v1.md`, plus the bounded DNG reader.
pub mod adjust;
pub mod cache;
pub mod catalog;
pub mod color;
pub mod detail;
pub mod develop;
pub mod dng;
pub mod dngout;
pub mod export;
pub mod icc;
pub mod jpeg;
pub mod lens;
pub mod ljpeg;
pub mod local;
pub mod math;
pub mod merge;
pub mod metadata;
pub mod opcode;
pub mod organize;
pub mod output;
pub mod pipeline;
pub mod pixels;
pub mod png;
pub mod profile;
pub mod raw;
pub mod studio;
pub mod tiff;
pub mod variants;
pub mod warp;

/// Document format version that adds the photography catalog and photo nodes.
pub const VERSION: u64 = 7;

/// Fail with `[cancelled]` when the current operation has been cancelled.
pub(crate) fn check_cancelled() -> anyhow::Result<()> {
    crate::composite::check_cancelled()
        .map_err(|_| anyhow::anyhow!("[cancelled] photo development cancelled"))
}

#[cfg(test)]
mod cancellation {
    //! Cancel decode, merge and export at every checkpoint they reach: each
    //! cancelled run must fail with `[cancelled]`, leave the document value
    //! unchanged and leave no output, staging directory or derived source.
    use super::{catalog, export};
    use crate::composite::cancel_at_check;
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn catalog(tag: &str) -> (Temp, PathBuf, Value) {
        let root = std::env::temp_dir().join(format!(
            "pentool-cancel-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let document = root.join("shoot.pen");
        let mut raw = crate::scene::new_document(40, 30);
        for (i, id) in ["b1", "b2", "b3"].into_iter().enumerate() {
            let bytes = crate::benchmark::bayer_dng(48, 32, i as u64);
            catalog::add_raw(
                &mut raw,
                id,
                id,
                &bytes,
                crate::image::embedded_storage(&bytes),
                catalog::camera_profile("auto").unwrap(),
            )
            .unwrap();
        }
        std::fs::write(&document, serde_json::to_vec(&raw).unwrap()).unwrap();
        (Temp(root.clone()), document, raw)
    }

    /// Run `attempt` cancelled at check 1, 2, ... until it completes; returns
    /// the number of checkpoints it passed.
    fn sweep(mut attempt: impl FnMut(usize) -> (anyhow::Result<()>, bool)) -> usize {
        for n in 1..10_000 {
            let (result, reached) = attempt(n);
            if !reached {
                result.expect("an uncancelled run succeeds");
                return n - 1;
            }
            let error = format!("{:#}", result.expect_err("a cancelled run fails"));
            assert!(error.contains("[cancelled]"), "check {n}: {error}");
        }
        panic!("the operation never completed")
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn decode_and_develop_stop_at_every_checkpoint() {
        let (_temp, document, raw) = catalog("decode");
        let expected = catalog::render_validated(&raw, &document, "b1", "master")
            .unwrap()
            .image
            .rgb;
        let checks = sweep(|n| {
            let (result, reached) = cancel_at_check(n, || {
                catalog::render_validated(&raw, &document, "b1", "master")
            });
            if !reached {
                assert_eq!(result.as_ref().unwrap().image.rgb, expected);
            }
            (result.map(|_| ()), reached)
        });
        assert!(checks >= 2, "decode and develop check for cancellation");
    }

    #[test]
    fn merges_leave_the_document_and_external_path_untouched() {
        let (temp, document, raw) = catalog("merge");
        let inputs = ["b1".to_owned(), "b2".to_owned(), "b3".to_owned()];
        let checks = sweep(|n| {
            let mut next = raw.clone();
            let (result, reached) = cancel_at_check(n, || {
                catalog::merge(
                    &mut next,
                    &document,
                    catalog::MergeRequest {
                        photo_id: "hdr",
                        name: "HDR",
                        inputs: &inputs,
                        kind: catalog::MergeKind::Hdr(super::merge::HdrOptions {
                            reference: None,
                            deghost: super::merge::Deghost::parse("off").unwrap(),
                            scale: 1.0,
                        }),
                        settings_first: false,
                        external: Some(PathBuf::from("merged/hdr.dng")),
                    },
                )
            });
            if reached {
                assert_eq!(next, raw, "a cancelled merge changed the document");
            } else {
                let (_, external) = result.as_ref().unwrap();
                assert!(external.is_some(), "the caller writes the derived source");
            }
            assert!(!temp.0.join("merged").exists());
            (result.map(|_| ()), reached)
        });
        assert!(checks >= 3, "merges check for cancellation");
    }

    #[test]
    fn exports_remove_staging_and_created_directories() {
        let (temp, document, raw) = catalog("export");
        let ids = vec!["b1".to_owned(), "b2".to_owned(), "b3".to_owned()];
        let recipe = export::Recipe::named(&raw, "archive-master").unwrap();
        // A fresh output directory, and one that already holds a file.
        let kept = temp.0.join("kept");
        std::fs::create_dir(&kept).unwrap();
        std::fs::write(kept.join("notes.txt"), b"keep").unwrap();
        for out in [temp.0.join("fresh"), kept.clone()] {
            let before = entries(&out);
            let planned = export::plan(
                &raw,
                &document,
                &recipe,
                &ids,
                &export::Variants::Master,
                &out,
                false,
            )
            .unwrap();
            let checks = sweep(|n| {
                let (result, reached) =
                    cancel_at_check(n, || export::run(&raw, &document, &recipe, &planned, &out));
                if reached {
                    assert_eq!(entries(&out), before, "check {n} left files behind");
                    assert_eq!(out.exists(), !before.is_empty() || out == kept);
                } else {
                    assert_eq!(entries(&out).len(), before.len() + planned.len());
                    for item in &planned {
                        std::fs::remove_file(out.join(&item.file)).unwrap();
                    }
                }
                (result.map(|_| ()), reached)
            });
            assert!(checks >= ids.len(), "every output checks for cancellation");
        }
    }
}
