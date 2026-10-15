use super::*;

fn source() -> SourceRepository {
    SourceRepository {
        repository_id: 42,
        repository: "organization/project".to_string(),
    }
}

const EXISTING: &str = r#"{"name":"tok","vers":"1.0.0"}"#;
const OWNER: &str = r#"{"repository_id":42,"repository":"old-owner/project"}"#;

#[test]
fn transferred_repository_keeps_publishing_rights() {
    assert!(!check_publish_state(Some(EXISTING), Some(OWNER), "tok", "1.1.0", &source()).unwrap());
}

#[test]
fn existing_version_is_rejected_before_upload() {
    assert!(
        check_publish_state(Some(EXISTING), Some(OWNER), "tok", "1.0.0", &source())
            .unwrap_err()
            .contains("already registered")
    );
}

#[test]
fn registered_pod_requires_matching_binding() {
    assert!(check_publish_state(Some(EXISTING), None, "tok", "1.1.0", &source()).is_err());
    let wrong = r#"{"repository_id":43,"repository":"organization/project"}"#;
    assert!(check_publish_state(Some(EXISTING), Some(wrong), "tok", "1.1.0", &source()).is_err());
}

#[test]
fn new_pod_requires_both_index_and_binding_to_be_absent() {
    assert!(check_publish_state(None, None, "tok", "1.0.0", &source()).unwrap());
    assert!(check_publish_state(None, Some(OWNER), "tok", "1.0.0", &source()).is_err());
}

#[test]
fn invalid_registry_data_fails_closed() {
    for content in ["", "not json", r#"{"name":"other","vers":"1.0.0"}"#] {
        assert!(
            check_publish_state(Some(content), Some(OWNER), "tok", "1.1.0", &source()).is_err()
        );
    }
    for binding in [
        "{}",
        r#"{"repository_id":0,"repository":"a/b"}"#,
        r#"{"repository_id":42,"repository":"a/b","extra":true}"#,
    ] {
        assert!(
            check_publish_state(Some(EXISTING), Some(binding), "tok", "1.1.0", &source()).is_err()
        );
    }
}

#[test]
fn append_preserves_published_record_bytes() {
    for content in [
        EXISTING.to_string(),
        format!("{EXISTING}\n"),
        format!(" {EXISTING}\n\n"),
    ] {
        let appended = append_record(&content, "new record");
        assert!(appended.starts_with(&content));
        assert_eq!(appended.lines().last(), Some("new record"));
    }
}

#[test]
fn repository_names_cannot_inject_paths_or_queries() {
    for name in [
        "",
        "owner",
        "a/b/c",
        "a/..",
        "a/b?ref=evil",
        "a/b#fragment",
        "a/b%2fc",
        "a/b\n",
    ] {
        assert!(!valid_repository_name(name), "accepted {name:?}");
    }
    assert!(valid_repository_name("owner/project-name.rs"));
}

#[test]
fn large_registry_file_uses_blob_fallback_with_exact_identity() {
    let content = "x".repeat(1024 * 1024 + 1);
    let metadata = json!({"type":"file", "sha":"abc123", "size":content.len(), "encoding":"none"});
    let (sha, decoded) = decode_registry_file(&metadata, "to/tok", |sha| {
        assert_eq!(sha, "abc123");
        Ok(json!({"sha":sha, "size":content.len(), "encoding":"base64", "content":B64.encode(&content)}))
    }).unwrap();
    assert_eq!(sha, "abc123");
    assert_eq!(decoded, content);
}

#[test]
fn registry_blob_limits_and_mismatches_fail_closed() {
    let metadata = json!({"type":"file", "sha":"abc123", "size":1, "encoding":"none"});
    for blob in [
        json!({"sha":"different", "size":1}),
        json!({"sha":"abc123", "size":2}),
        json!({"sha":"abc123", "size":1, "encoding":"none"}),
        json!({"sha":"abc123", "size":1, "encoding":"base64", "content":"!"}),
        json!({"sha":"abc123", "size":1, "encoding":"base64", "content":B64.encode("xx")}),
        json!({"sha":"abc123", "size":1, "encoding":"base64", "content":B64.encode([255])}),
    ] {
        assert!(decode_registry_file(&metadata, "to/tok", |_| Ok(blob)).is_err());
    }
    for value in [
        json!({"type":"dir", "sha":"abc123", "size":1}),
        json!({"type":"file", "sha":"abc123", "size":MAX_METADATA_BYTES + 1}),
        json!({"type":"file", "sha":"abc123", "size":-1}),
        json!({"type":"file", "sha":"abc123", "size":1, "encoding":"utf8"}),
    ] {
        assert!(
            decode_registry_file(&value, "to/tok", |_| panic!("unexpected blob request")).is_err()
        );
    }
}

#[test]
fn fork_parent_must_match_for_existing_and_new_forks() {
    let upstream = json!({"id":42});
    assert!(validate_fork(&json!({"parent":{"id":42}}), &upstream, "user/pit-registry").is_ok());
    for fork in [json!({}), json!({"parent":{"id":43}})] {
        assert!(validate_fork(&fork, &upstream, "user/pit-registry").is_err());
    }
}
