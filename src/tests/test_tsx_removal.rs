use super::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A RAII-based test environment helper to manage temporary file operations
/// without polluting the host workspace or relying on external dependencies.
struct TestEnv {
    sandbox_dir: PathBuf,
}

impl TestEnv {
    fn new(test_name: &str) -> Self {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("fitseal_test_{}_{}", test_name, nanos));
        fs::create_dir_all(&path).expect("Failed to create temporary test directory");
        Self { sandbox_dir: path }
    }

    fn write_file(&self, relative_path: &str, content: &str) -> PathBuf {
        let full_path = self.sandbox_dir.join(relative_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent directories");
        }
        fs::write(&full_path, content).expect("Failed to write test file");
        full_path
    }

    fn read_file(&self, relative_path: &str) -> String {
        let full_path = self.sandbox_dir.join(relative_path);
        fs::read_to_string(full_path).expect("Failed to read test file")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.sandbox_dir);
    }
}

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

#[test]
fn test_single_update_fuzzy_alignment() {
    let env = TestEnv::new("single_update_fuzzy");
    
    // Original target file content on disk (contains a slightly different comment to test drift resilience)
    let original_content = r#"
fn run_calculations() {
    // Perform safety checks
    let a = 10;
    let b = 20;
    println!("result: {}", a + b);
}
"#;
    let file_path = env.write_file("calc.rs", original_content);
    let file_path_str = file_path.to_str().unwrap();

    // AI suggestion has slightly different formatting/comments in prefix, but sequence alignment should resolve it
    let raw_update_block = r#"
<update>
<file>TARGET_FILE_PLACEHOLDER</file>
<![CDATA[
fn run_calculations() {
    // Perform safety check drift
@@@
    let a = 50;
    let b = 100;
@@@
    println!("result: {}", a + b);
}
]]>
</update>
"#;
    let update_block = raw_update_block.replace("TARGET_FILE_PLACEHOLDER", file_path_str);

    let mut cache = HashSet::new();
    let success = process_instruction_text(&update_block, true, false, &mut cache);

    assert!(success, "Instruction execution returned an error status");
    
    let updated_content = env.read_file("calc.rs");
    assert!(updated_content.contains("let a = 50;"));
    assert!(updated_content.contains("let b = 100;"));
    assert!(!updated_content.contains("let a = 10;"));
}

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

#[test]
fn test_tsx_trigger_removal() {
    let env = TestEnv::new("tsx_trigger_removal");

    let original_content = r#"import { component$, useSignal, useTask$, useContext, $ } from '@qwik.dev/core';
import { isBrowser } from '@qwik.dev/core/build';
import { Link } from '@qwik.dev/router';
import { CollapsibleTrigger } from '~primitives/@kit/collapsible';

import { Icon } from '~/utils/icon';

import { ChangelogContext } from '.';

pub const ChangelogTrigger = component$(() => {
  const ctx = useContext(ChangelogContext);
  const latest = ctx.latestItem;

  const hasInteracted = useSignal(false);

  useTask$(({ track }) => {
    track(() => ctx.isExpanded.value);
    if (isBrowser) {
      hasInteracted.value = true;
    }
  });

  const animationClass = hasInteracted.value ? 'animate-in-place' : '';

  return (
    <span
      class={[
        'w-full relative flex-shrink-0 h-[44px] z-30',
        !ctx.isExpanded.value &&
        'flex items-center justify-between bg-transparent rounded-[19.5px]',
      ]}
    >
      <style>{`
        @keyframes blurIn {
          from { opacity: 0; filter: blur(4px); }
          to { opacity: 1; filter: blur(0px); }
        }
        .animate-in-place {
          animation: blurIn var(--duration-slower) var(--ease-spring-slow) forwards;
        }
      `}</style>

      <Link
        href={latest.href}
        class={[
          'group/item flex-1 items-center justify-start pl-6 pr-4 h-full outline-none rounded-l-[19.5px] cursor-pointer focus-visible:ring-1 focus-visible:ring-separator focus-visible:bg-canvas-hover transition-colors overflow-hidden leading-[1] gap-2',
          'spring-slower-exit',
          `group-hover/item:spring-slower`,
          !ctx.isExpanded.value ? 'flex' : 'hidden',
          !ctx.isExpanded.value && animationClass,
        ]}
      >
        <p
          class={[
            'relative text-ink-tertiary leading-[1] z-10 group-hover/item:text-ink group-focus-visible/item:text-ink transition-colors text-sm',
            'spring-slower-exit',
            `group-hover/item:spring-slower`,
          ]}
        >
          {latest.version}
        </p>

        <p
          class={[
            'flex items-center gap-2 font-fvs text-sm tracking-wide group-hover/item:fvs-medium group-focus-visible/item:fvs-medium group-hover/item:underline font-synthesis-none transition-all',
            'spring-slower-exit',
            `group-hover/item:spring-slower`,
          ]}
        >
          {latest.title}
          <Icon
            i="arrow-up-right"
            class={[
              'text-ink-tertiary opacity-0 scale-90 -translate-x-1 translate-y-1 group-hover/item:opacity-100 group-hover/item:scale-100 group-hover/item:translate-x-0 group-hover/item:translate-y-0 transition-all shrink-0',
              'spring-slower-exit',
              `group-hover/item:spring-slower`,
            ]}
            size="sm"
          />
        </p>

        {latest.newBadge && (
          <span class="px-2 rounded-md ml-auto py-1 bg-canvas-contrast text-macro text-ink-contrast tracking-tight font-semibold">
            NEW
          </span>
        )}
      </Link>

      {/* Vertical Separator */}
      <span
        class={[
          'w-[1px] h-[20px] bg-separator-box shrink-0',
          !ctx.isExpanded.value ? 'block' : 'hidden',
        ]}
      />

      <CollapsibleTrigger
        onClick$={$(() => {
          ctx.isExpanded.value = !ctx.isExpanded.value;
        })}
        class={[
          'outline-none cursor-pointer focus-visible:ring-1 focus-visible:ring-separator focus-visible:bg-canvas-hover',
          ctx.isExpanded.value
            ? 'w-full h-full flex items-center justify-between px-6 bg-transparent hover:bg-canvas-hover rounded-[19.5px]'
            : 'group/trigger flex items-center gap-1 pr-6 pl-4 h-full rounded-r-[19.5px] text-ink-tertiary hover:text-ink hover:underline',
        ]}
      >
        {/* Expanded Content View (Hidden when collapsed) */}
        <span
          class={[
            'items-center justify-between w-full h-full',
            ctx.isExpanded.value ? 'flex' : 'hidden',
          ]}
        >
          <span class={['flex items-center gap-2', ctx.isExpanded.value && animationClass]}>
            <span class="text-sm text-ink-secondary">Changelog</span>
          </span>
          <Icon
            i="chevron-down"
            class={[
              'text-ink-tertiary transition-transform rotate-180',
              'spring-slower-exit',
              `group-hover/trigger:spring-slower`,
            ]}
            size="sm"
          />
        </span>

        {/* Collapsed Content View (Hidden when expanded) */}
        <span class={['items-center gap-1 h-full', !ctx.isExpanded.value ? 'flex' : 'hidden']}>
          <span class="text-xs font-medium">View more</span>
          <Icon
            i="chevron-down"
            class={[
              'transition-transform group-hover/trigger:translate-y-[2px]',
              'spring-slower-exit',
              `group-hover/trigger:spring-slower`,
            ]}
            size="sm"
          />
        </span>
      </CollapsibleTrigger>
    </span>
  );
});
"#;

    let file_path = env.write_file("trigger.tsx", original_content);
    let file_path_str = file_path.to_str().unwrap();

    let raw_payload = r#"
<update>
<file>TARGET_FILE_PLACEHOLDER</file>
<![CDATA[
      {/* Vertical Separator */}
      <span
        class={[
          'w-[1px] h-[20px] bg-separator-box shrink-0',
          !ctx.isExpanded.value ? 'block' : 'hidden',
        ]}
      />

      <CollapsibleTrigger
        onClick$={$(() => {
          ctx.isExpanded.value = !ctx.isExpanded.value;
        })}
        class={[
          'outline-none cursor-pointer focus-visible:ring-1 focus-visible:ring-separator focus-visible:bg-canvas-hover',
          ctx.isExpanded.value
@@@
      {/* Vertical Separator */}
      <span
        class={[
          'w-[1px] h-[20px] bg-separator-box shrink-0',
          !ctx.isExpanded.value ? 'block' : 'hidden',
        ]}
      />

      <CollapsibleTrigger
        class={[
          'outline-none cursor-pointer focus-visible:ring-1 focus-visible:ring-separator focus-visible:bg-canvas-hover',
          ctx.isExpanded.value
@@@
]]>
</update>
"#;

    let payload = raw_payload.replace("TARGET_FILE_PLACEHOLDER", file_path_str);

    let mut cache = HashSet::new();
    let success = process_instruction_text(&payload, true, false, &mut cache);

    assert!(success, "TSX update processing returned an error status");

    let final_content = env.read_file("trigger.tsx");
    assert!(!final_content.contains("onClick$={$(() => {"));
    assert!(final_content.contains("pub const ChangelogTrigger = component$(() => {"));
}
