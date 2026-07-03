mod common;

use common::TestEnv;
use fitseal::process_instruction_text;
use std::collections::HashSet;

#[test]
fn test_multiple_sequential_updates_same_file_aggregation() {
    let env = TestEnv::new("multi_update_aggregation");

    let original_content = r#"
fn sequence() {
    step_one();
    step_two();
    step_three();
}
"#;
    let file_path = env.write_file("seq.rs", original_content);
    let file_path_str = file_path.to_str().unwrap();

    // Payload containing two separate update operations targeting the same file
    let raw_multi_update_payload = r#"
<update>
<file>TARGET_FILE_PLACEHOLDER</file>
<![CDATA[
fn sequence() {
@@@
    step_one_modified();
@@@
    step_two();
    step_three();
}
]]>
</update>

<update>
<file>TARGET_FILE_PLACEHOLDER</file>
<![CDATA[
fn sequence() {
    step_one_modified();
    step_two();
@@@
    step_two_modified();
    step_two_extra();
@@@
    step_three();
}
]]>
</update>
"#;
    let multi_update_payload = raw_multi_update_payload.replace("TARGET_FILE_PLACEHOLDER", file_path_str);

    let mut cache = HashSet::new();
    let success = process_instruction_text(&multi_update_payload, true, false, &mut cache);

    assert!(success, "Multi-update execution returned an error status");

    let final_content = env.read_file("seq.rs");
    
    // Ensure both modifications were sequentially applied to the in-memory document state
    // and correctly written without overwriting each other
    assert!(final_content.contains("step_one_modified();"));
    assert!(final_content.contains("step_two_modified();"));
    assert!(final_content.contains("step_two_extra();"));
    assert!(final_content.contains("step_three();"));
    assert!(!final_content.contains("step_one();"));
    assert!(!final_content.contains("step_two();\n    step_three();"));
}

#[test]
fn test_mixed_create_and_update_chain() {
    let env = TestEnv::new("mixed_create_update");
    let file_path = env.sandbox_dir.join("new_module.rs");
    let file_path_str = file_path.to_str().unwrap();

    let raw_payload = r#"
<create>
<file>TARGET_FILE_PLACEHOLDER</file>
<content><![CDATA[
pub fn init() {
    println!("uninitialized");
}
]]></content>
</create>

<update>
<file>TARGET_FILE_PLACEHOLDER</file>
<![CDATA[
pub fn init() {
@@@
    println!("initializing...");
    setup_logging();
@@@
}
]]>
</update>
"#;
    let payload = raw_payload.replace("TARGET_FILE_PLACEHOLDER", file_path_str);

    let mut cache = HashSet::new();
    let success = process_instruction_text(&payload, true, false, &mut cache);

    assert!(success, "Mixed transaction processing returned an error status");

    let final_content = env.read_file("new_module.rs");
    assert!(final_content.contains("pub fn init() {"));
    assert!(final_content.contains("setup_logging();"));
    assert!(!final_content.contains("uninitialized"));
}
