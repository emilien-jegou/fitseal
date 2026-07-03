use fitseal::{parse_instructions, Instruction};

#[test]
fn test_parse_valid_update_block() {
    let input = r#"
<update>
<file>src/main.rs</file>
<![CDATA[
fn main() {
    println!("hello");
@@@
    println!("hello world");
    let x = 42;
@@@
}
]]>
</update>
"#;

    let instructions = parse_instructions(input);
    assert_eq!(instructions.len(), 1);

    if let Instruction::Update {
        file_target,
        prefix,
        replacement,
        suffix,
    } = &instructions[0]
    {
        assert_eq!(file_target, "src/main.rs");
        assert_eq!(prefix, "fn main() {\n    println!(\"hello\");\n");
        assert_eq!(replacement, "    println!(\"hello world\");\n    let x = 42;\n");
        assert_eq!(suffix, "}\n");
    } else {
        panic!("Parsed instruction was not of variant Instruction::Update");
    }
}

#[test]
fn test_parse_malformed_update_block_missing_separators() {
    let input = r#"
<update>
<file>src/main.rs</file>
fn main() {
    println!("hello");
    // Missing @@@ markers entirely
    println!("hello world");
}
</update>
"#;

    let instructions = parse_instructions(input);
    // Should gracefully ignore/skip invalid structures lacking exactly two separators
    assert!(instructions.is_empty());
}
