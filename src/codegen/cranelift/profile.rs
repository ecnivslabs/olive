//! PGO profile: what a JIT run observed, fed into a later AOT compile via
//! `pit build --pgo=<path>`, both sharing `CraneliftCodegen<M>`.

use super::{ANY_SITE_GRADUATED, CraneliftCodegen};
use cranelift_jit::JITModule;
use cranelift_module::Module;
use rustc_hash::FxHashMap as HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct FunctionProfile {
    pub(crate) hotcount: i64,
    /// Kind-history byte per site, source order; same encoding as `std_lib`'s runtime.
    pub(crate) any_add_sites: Vec<u8>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Profile {
    pub(crate) functions: HashMap<String, FunctionProfile>,
}

impl CraneliftCodegen<JITModule> {
    /// Keyed by name, not position -- a rebuild can add/remove/reorder functions.
    pub(crate) fn export_profile(&mut self) -> Profile {
        let names: Vec<String> = self.hotcount_ids.keys().cloned().collect();
        let mut functions: HashMap<String, FunctionProfile> = names
            .into_iter()
            .map(|name| {
                let hotcount = self.hotcount(&name).unwrap_or(0);
                (
                    name,
                    FunctionProfile {
                        hotcount,
                        any_add_sites: Vec::new(),
                    },
                )
            })
            .collect();

        let ranges: Vec<(String, usize, usize)> = self
            .any_add_site_ranges
            .iter()
            .map(|(name, &(start, end))| (name.clone(), start, end))
            .collect();
        for (name, start, end) in ranges {
            let sites: Vec<u8> = (start..end)
                .map(|i| self.any_add_site_kind(i).unwrap_or(0))
                .collect();
            functions.entry(name).or_default().any_add_sites = sites;
        }

        Profile { functions }
    }
}

impl<M: Module> CraneliftCodegen<M> {
    /// Seeds `specialize_sites` before `generate()`. Site-count mismatch (stale
    /// profile, source changed) skips that function entirely, not partially.
    /// Returns count applied, so a caller can report a real number.
    pub(crate) fn apply_profile(&mut self, profile: &Profile) -> usize {
        let (_, ranges) = self.count_any_add_sites();
        let mut applied = 0;
        for (name, (start, end)) in ranges {
            let Some(fp) = profile.functions.get(&name) else {
                continue;
            };
            if fp.any_add_sites.len() != end - start {
                continue;
            }
            for (i, &kind) in fp.any_add_sites.iter().enumerate() {
                if kind == ANY_SITE_GRADUATED && self.specialize_sites.insert(start + i) {
                    applied += 1;
                }
            }
        }
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{call_i64_1, compile_minimal, compile_minimal_aot};

    const SRC: &str = concat!(
        "fn f(a: Any, b: Any) -> Any:\n",
        "    return a + b\n",
        "\n",
        "fn driver(n: i64) -> i64:\n",
        "    let r = f(n, n)\n",
        "    return int(r)\n",
    );

    #[test]
    fn export_profile_captures_hotcount_and_graduated_site() {
        let mut cg = compile_minimal(SRC);
        for i in 0..10i64 {
            call_i64_1(&mut cg, "driver", i);
        }
        let profile = cg.export_profile();
        assert_eq!(profile.functions["driver"].hotcount, 10);
        assert_eq!(profile.functions["f"].hotcount, 10);
        assert_eq!(profile.functions["f"].any_add_sites, vec![8]);
    }

    #[test]
    fn export_profile_captures_ungraduated_site() {
        let mut cg = compile_minimal(SRC);
        for i in 0..3i64 {
            call_i64_1(&mut cg, "driver", i);
        }
        let profile = cg.export_profile();
        assert_eq!(profile.functions["f"].any_add_sites, vec![3]);
    }

    #[test]
    fn apply_profile_specializes_matching_site() {
        let profile = Profile {
            functions: HashMap::from_iter([(
                "f".to_string(),
                FunctionProfile {
                    hotcount: 1000,
                    any_add_sites: vec![8],
                },
            )]),
        };
        let mut cg = compile_minimal_aot(SRC);
        assert_eq!(cg.apply_profile(&profile), 1);
        assert!(cg.specialize_sites.contains(&0));
    }

    #[test]
    fn apply_profile_skips_ungraduated_site() {
        let profile = Profile {
            functions: HashMap::from_iter([(
                "f".to_string(),
                FunctionProfile {
                    hotcount: 5,
                    any_add_sites: vec![3],
                },
            )]),
        };
        let mut cg = compile_minimal_aot(SRC);
        assert_eq!(cg.apply_profile(&profile), 0);
        assert!(cg.specialize_sites.is_empty());
    }

    #[test]
    fn apply_profile_skips_stale_site_count_mismatch() {
        let profile = Profile {
            functions: HashMap::from_iter([(
                "f".to_string(),
                // Source now has 1 site, profile was captured against 2.
                FunctionProfile {
                    hotcount: 1000,
                    any_add_sites: vec![8, 8],
                },
            )]),
        };
        let mut cg = compile_minimal_aot(SRC);
        assert_eq!(cg.apply_profile(&profile), 0);
        assert!(cg.specialize_sites.is_empty());
    }

    #[test]
    fn apply_profile_ignores_unknown_function() {
        let profile = Profile {
            functions: HashMap::from_iter([(
                "does_not_exist".to_string(),
                FunctionProfile {
                    hotcount: 1000,
                    any_add_sites: vec![8],
                },
            )]),
        };
        let mut cg = compile_minimal_aot(SRC);
        assert_eq!(cg.apply_profile(&profile), 0);
        assert!(cg.specialize_sites.is_empty());
    }

    #[test]
    fn round_trip_through_json() {
        let mut cg = compile_minimal(SRC);
        for i in 0..10i64 {
            call_i64_1(&mut cg, "driver", i);
        }
        let profile = cg.export_profile();
        let json = serde_json::to_string(&profile).unwrap();
        let restored: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.functions["f"].any_add_sites, vec![8]);
        assert_eq!(restored.functions["driver"].hotcount, 10);
    }
}
