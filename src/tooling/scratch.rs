use pubgrub::solver::{resolve, OfflineDependencyProvider};
use pubgrub::range::Range;
use pubgrub::version::SemanticVersion;

pub fn test() {
    let mut provider = OfflineDependencyProvider::<String, SemanticVersion>::new();
    
    let v1 = SemanticVersion::from((1, 0, 0));
    let r1 = Range::exact(v1.clone());
    
    provider.add_dependencies(
        "root".to_string(),
        v1,
        vec![("dep".to_string(), r1)]
    );
}
