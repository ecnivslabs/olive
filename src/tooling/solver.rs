use pubgrub::{OfflineDependencyProvider, Range, resolve};
use std::collections::HashMap;

use crate::tooling::registry::{PodVersion, fetch_versions};

#[derive(Debug)]
pub enum SolverError {
    Unresolvable(String),
    Registry(String),
}

impl std::fmt::Display for SolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SolverError::Unresolvable(msg) => write!(f, "dependency resolution failed: {}", msg),
            SolverError::Registry(msg) => write!(f, "registry error: {}", msg),
        }
    }
}
impl std::error::Error for SolverError {}

impl From<String> for SolverError {
    fn from(s: String) -> Self {
        SolverError::Registry(s)
    }
}

pub async fn resolve_tree(
    root_deps: &HashMap<String, String>,
    locked_deps: Option<HashMap<String, String>>,
    offline: bool,
) -> Result<Vec<PodVersion>, SolverError> {
    let mut registry_cache = HashMap::new();
    let mut to_fetch = Vec::new();
    for name in root_deps.keys() {
        to_fetch.push(name.clone());
    }

    while !to_fetch.is_empty() {
        let mut in_flight = Vec::new();
        let mut batch = std::collections::HashSet::new();

        while let Some(name) = to_fetch.pop() {
            if !registry_cache.contains_key(&name) && batch.insert(name.clone()) {
                in_flight.push(async move {
                    let res = fetch_versions(&name, offline).await;
                    (name, res)
                });
            }
        }

        let results = futures::future::join_all(in_flight).await;

        for (name, res) in results {
            let versions = res?;
            for v in &versions {
                for dep in &v.deps {
                    if !registry_cache.contains_key(&dep.name) && !batch.contains(&dep.name) {
                        to_fetch.push(dep.name.clone());
                    }
                }
            }
            registry_cache.insert(name, versions);
        }
    }

    let mut provider = OfflineDependencyProvider::<String, Range<semver::Version>>::new();
    let mut pod_versions_map: HashMap<(String, semver::Version), PodVersion> = HashMap::new();

    for (name, versions) in &registry_cache {
        for v in versions {
            if v.yanked {
                continue;
            }
            if let Ok(semver_parsed) = semver::Version::parse(&v.vers) {
                pod_versions_map.insert((name.clone(), semver_parsed.clone()), v.clone());

                let mut deps_list = Vec::new();
                for dep in &v.deps {
                    let range = parse_range(&dep.req);
                    deps_list.push((dep.name.clone(), range));
                }

                provider.add_dependencies(name.clone(), semver_parsed, deps_list);
            }
        }
    }

    let root_sv = semver::Version::new(0, 0, 0);
    let mut root_reqs = Vec::new();
    for (name, req) in root_deps {
        root_reqs.push((name.clone(), parse_range(req)));
    }
    if let Some(locked) = locked_deps {
        for (name, vers) in locked {
            if let Ok(p) = semver::Version::parse(&vers) {
                root_reqs.push((name.clone(), Range::singleton(p)));
            }
        }
    }

    provider.add_dependencies("root".to_string(), root_sv.clone(), root_reqs);

    match resolve(&provider, "root".to_string(), root_sv) {
        Ok(solution) => {
            let mut final_pods = Vec::new();
            for (pkg, version) in solution {
                if pkg == "root" {
                    continue;
                }
                if let Some(pod_version) = pod_versions_map.get(&(pkg.clone(), version.clone())) {
                    final_pods.push(pod_version.clone());
                }
            }
            Ok(final_pods)
        }
        Err(e) => Err(SolverError::Unresolvable(format!("{:?}", e))),
    }
}

fn parse_range(req: &str) -> Range<semver::Version> {
    if req == "*" || req == "latest" {
        return Range::full();
    }
    if let Ok(v) = semver::Version::parse(req) {
        return Range::singleton(v);
    }
    Range::full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_wildcard_is_full() {
        let r = parse_range("*");
        assert_eq!(r, Range::full());
    }

    #[test]
    fn parse_range_latest_is_full() {
        let r = parse_range("latest");
        assert_eq!(r, Range::full());
    }

    #[test]
    fn parse_range_exact_version_is_singleton() {
        let r = parse_range("1.2.3");
        let expected = Range::singleton(semver::Version::new(1, 2, 3));
        assert_eq!(r, expected);
    }

    #[test]
    fn parse_range_invalid_is_full() {
        let r = parse_range("not-a-version");
        assert_eq!(r, Range::full());
    }

    #[test]
    fn parse_range_partial_version_falls_back_to_full() {
        let r = parse_range("1.2");
        assert_eq!(r, Range::full());
    }

    #[test]
    fn solver_error_unresolvable_display() {
        let e = SolverError::Unresolvable("conflict between A >=1.0 and B <0.5".into());
        let msg = format!("{e}");
        assert!(msg.contains("dependency resolution failed"));
        assert!(msg.contains("conflict between"));
    }

    #[test]
    fn solver_error_registry_display() {
        let e = SolverError::Registry("connection refused".into());
        let msg = format!("{e}");
        assert!(msg.contains("registry error"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn solver_error_from_string() {
        let e: SolverError = "timeout".to_string().into();
        match e {
            SolverError::Registry(msg) => assert_eq!(msg, "timeout"),
            _ => panic!("expected Registry variant"),
        }
    }
}
