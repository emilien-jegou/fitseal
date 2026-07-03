use arboard::Clipboard;
use clap::{Parser, Subcommand};
use colored::Colorize;
use ignore::WalkBuilder;
use rayon::prelude::*;
use similar::TextDiff;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info, trace, warn};

#[derive(Parser)]
#[command(name = "fitseal")]
#[command(about = "Fuzzy find-and-replace using sequence alignment", long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub log_file: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Daemon {
        #[arg(long)]
        auto_apply: bool,
    },
    Apply {
        file: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Hash, Clone, Debug, PartialEq)]
pub enum Instruction {
    Update {
        file_target: String,
        prefix: String,
        replacement: String,
        suffix: String,
    },
    Create {
        file_target: String,
        content: String,
    },
    Delete {
        file_target: String,
    },
}

pub struct FileState {
    pub resolved_path: PathBuf,
    pub original_content: String,
    pub current_content: String,
    pub action_char: char, // 'A', 'M', 'D'
    pub update_count: usize,
    pub confidences: Vec<f32>,
    pub hashes: Vec<u64>,
}

pub fn contains_instruction(text: &str) -> bool {
    (text.contains("<update>") && text.contains("</update>"))
        || (text.contains("<create>") && text.contains("</create>"))
        || (text.contains("<delete>") && text.contains("</delete>"))
}

pub fn run_daemon(auto_apply: bool) {
    info!("Initializing clipboard listener...");
    println!(
        "{} Fitseal daemon started. Watching clipboard for instruction blocks...",
        "🦭".cyan()
    );

    let mut clipboard = Clipboard::new().unwrap_or_else(|e| {
        error!("Clipboard initialization error: {}", e);
        eprintln!(
            "{} Failed to initialize clipboard: {}",
            "✖ Error:".red().bold(),
            e
        );
        std::process::exit(1);
    });

    let mut last_content = String::new();
    let mut cache: HashSet<u64> = HashSet::new();

    loop {
        if let Ok(text) = clipboard.get_text() {
            if text != last_content && contains_instruction(&text) {
                debug!("New instruction sequence found in clipboard");
                last_content = text.clone();
                process_instruction_text(&text, auto_apply, false, &mut cache);
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
}

pub fn process_instruction_text(
    text: &str,
    auto_apply: bool,
    dry_run: bool,
    cache: &mut HashSet<u64>,
) -> bool {
    let instructions = parse_instructions(text);
    if instructions.is_empty() {
        debug!("No valid instruction blocks found after parsing.");
        return false;
    }

    info!("Evaluating {} parsed instruction(s)...", instructions.len());

    let mut file_states: HashMap<PathBuf, FileState> = HashMap::new();
    let mut resolved_targets: HashMap<String, PathBuf> = HashMap::new();
    let mut failures = Vec::new();

    for instruction in instructions {
        let mut hasher = DefaultHasher::new();
        instruction.hash(&mut hasher);
        let inst_hash = hasher.finish();

        if cache.contains(&inst_hash) {
            debug!("Instruction hash {:x} found in session cache. Skipping.", inst_hash);
            continue;
        }

        match &instruction.clone() {
            Instruction::Update {
                file_target,
                prefix,
                replacement,
                suffix,
            } => {
                debug!("Evaluating 'Update' for target: {}", file_target);

                // 1. Resolve target path
                let path = if let Some(p) = resolved_targets.get(file_target) {
                    p.clone()
                } else {
                    let mut pattern = String::new();
                    pattern.push_str(prefix);
                    if !prefix.is_empty() && !prefix.ends_with('\n') {
                        pattern.push('\n');
                    }
                    pattern.push_str("// ...\n");
                    pattern.push_str(suffix);

                    let pattern_lines = to_line_info(&pattern);
                    match find_best_target_file(file_target, &pattern_lines) {
                        Some((p, _, _, _)) => {
                            resolved_targets.insert(file_target.clone(), p.clone());
                            p
                        }
                        None => {
                            warn!("No file match found for target: {}", file_target);
                            failures.push((
                                instruction.clone(),
                                "Target file or pattern match not found on disk".to_string(),
                            ));
                            continue;
                        }
                    }
                };

                // 2. Retrieve or initialize the FileState
                let state = if let Some(s) = file_states.get_mut(&path) {
                    s
                } else {
                    let original_content = match fs::read_to_string(&path) {
                        Ok(content) => content,
                        Err(e) => {
                            error!("Failed to read file {}: {}", path.display(), e);
                            failures.push((
                                instruction.clone(),
                                format!("Failed to read file: {}", e),
                            ));
                            continue;
                        }
                    };
                    let init_state = FileState {
                        resolved_path: path.clone(),
                        original_content: original_content.clone(),
                        current_content: original_content,
                        action_char: 'M',
                        update_count: 0,
                        confidences: Vec::new(),
                        hashes: Vec::new(),
                    };
                    file_states.insert(path.clone(), init_state);
                    file_states.get_mut(&path).unwrap()
                };

                // 3. Find block pattern in current content of state
                let mut pattern = String::new();
                pattern.push_str(prefix);
                if !prefix.is_empty() && !prefix.ends_with('\n') {
                    pattern.push('\n');
                }
                pattern.push_str("// ...\n");
                pattern.push_str(suffix);

                let pattern_lines = to_line_info(&pattern);
                let current_lines = to_line_info(&state.current_content);

                match fuzzy_find_block(&state.current_content, &current_lines, &pattern_lines) {
                    Some((byte_range, confidence, _elisions)) => {
                        if confidence < 0.4 {
                            warn!(
                                "Match confidence {:.2}% for {} is below threshold",
                                confidence * 100.0,
                                file_target
                            );
                            failures.push((
                                instruction.clone(),
                                format!("Confidence {:.2}% is too low", confidence * 100.0),
                            ));
                            continue;
                        }

                        // Apply the update locally to state.current_content
                        let mut merged_replacement = String::new();
                        merged_replacement.push_str(prefix);
                        merged_replacement.push_str(replacement);
                        merged_replacement.push_str(suffix);

                        let mut new_content = String::with_capacity(
                            state.current_content.len() + merged_replacement.len(),
                        );
                        new_content.push_str(&state.current_content[..byte_range.start]);
                        new_content.push_str(&merged_replacement);
                        new_content.push_str(&state.current_content[byte_range.end..]);

                        state.current_content = new_content;
                        state.update_count += 1;
                        state.confidences.push(confidence);
                        state.hashes.push(inst_hash);
                    }
                    None => {
                        warn!("Pattern match not found inside the current in-memory content for target: {}", file_target);
                        failures.push((
                            instruction.clone(),
                            "Pattern match not found in the current content of the file".to_string(),
                        ));
                    }
                }
            }
            Instruction::Create {
                file_target,
                content,
            } => {
                debug!("Evaluating 'Create' for target: {}", file_target);
                let path = PathBuf::from(file_target);
                resolved_targets.insert(file_target.clone(), path.clone());

                let state = if let Some(s) = file_states.get_mut(&path) {
                    s
                } else {
                    let exists = path.exists();
                    let original_content = if exists {
                        match fs::read_to_string(&path) {
                            Ok(c) => c,
                            Err(e) => {
                                error!("Failed to read existing file {}: {}", path.display(), e);
                                failures.push((
                                    instruction.clone(),
                                    format!("Failed to read existing file: {}", e),
                                ));
                                continue;
                            }
                        }
                    } else {
                        String::new()
                    };
                    let init_state = FileState {
                        resolved_path: path.clone(),
                        original_content: original_content.clone(),
                        current_content: original_content,
                        action_char: if exists { 'M' } else { 'A' },
                        update_count: 0,
                        confidences: Vec::new(),
                        hashes: Vec::new(),
                    };
                    file_states.insert(path.clone(), init_state);
                    file_states.get_mut(&path).unwrap()
                };

                state.current_content = content.clone();
                state.update_count += 1;
                state.hashes.push(inst_hash);
            }
            Instruction::Delete { file_target } => {
                debug!("Evaluating 'Delete' for target: {}", file_target);
                let path = if let Some(p) = resolved_targets.get(file_target) {
                    p.clone()
                } else {
                    match find_existing_file(file_target) {
                        Some(p) => {
                            resolved_targets.insert(file_target.clone(), p.clone());
                            p
                        }
                        None => {
                            warn!("Delete target file not found on disk: {}", file_target);
                            failures.push((
                                instruction.clone(),
                                "File to delete not found".to_string(),
                            ));
                            continue;
                        }
                    }
                };

                let state = if let Some(s) = file_states.get_mut(&path) {
                    s
                } else {
                    let original_content = match fs::read_to_string(&path) {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to read file to delete {}: {}", path.display(), e);
                            failures.push((
                                instruction.clone(),
                                format!("Failed to read file: {}", e),
                            ));
                            continue;
                        }
                    };
                    let init_state = FileState {
                        resolved_path: path.clone(),
                        original_content: original_content.clone(),
                        current_content: original_content,
                        action_char: 'D',
                        update_count: 0,
                        confidences: Vec::new(),
                        hashes: Vec::new(),
                    };
                    file_states.insert(path.clone(), init_state);
                    file_states.get_mut(&path).unwrap()
                };

                state.current_content = String::new();
                state.action_char = 'D';
                state.update_count += 1;
                state.hashes.push(inst_hash);
            }
        }
    }

    if !failures.is_empty() {
        println!("\n{}", "✖ Resolve Failures:".red().bold());
        for (inst, reason) in &failures {
            let target = match inst {
                Instruction::Update { file_target, .. } => file_target,
                Instruction::Create { file_target, .. } => file_target,
                Instruction::Delete { file_target, .. } => file_target,
            };
            println!("  - {}: {}", target.magenta(), reason.red());
        }
    }

    let valid_states: Vec<&FileState> = file_states
        .values()
        .filter(|s| !s.hashes.is_empty())
        .collect();

    if valid_states.is_empty() {
        info!("No successfully prepared actions to run.");
        return false;
    }

    println!("\n{}", "@@ FOUND MATCH @@".bold().cyan());

    // Print Detailed Diffs (Consolidated per unique file)
    println!("\n{}", "Detailed Diffs:".bold());
    for state in &valid_states {
        println!(
            "\n--- {} ---",
            state.resolved_path.display().to_string().cyan().bold()
        );
        print_diff(&state.original_content, &state.current_content, &state.resolved_path);
    }

    // Print Revision Summary
    println!("\n{}", "Proposed Revision Summary:".bold().cyan());
    for state in &valid_states {
        let diff = TextDiff::from_lines(&state.original_content, &state.current_content);
        let mut lines_added = 0;
        let mut lines_removed = 0;
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Insert => lines_added += 1,
                similar::ChangeTag::Delete => lines_removed += 1,
                similar::ChangeTag::Equal => {}
            }
        }

        let action_colored = match state.action_char {
            'A' => "A".green().bold(),
            'M' => "M".yellow().bold(),
            'D' => "D".red().bold(),
            _ => state.action_char.to_string().normal(),
        };

        let mut stats = String::new();
        if lines_added > 0 {
            stats.push_str(&format!("+{}", lines_added).green());
        }
        if lines_removed > 0 {
            if !stats.is_empty() {
                stats.push(' ');
            }
            stats.push_str(&format!("-{}", lines_removed).red());
        }

        let mut details = Vec::new();
        if state.update_count > 1 {
            details.push(format!("{} updates", state.update_count));
        }
        if !state.confidences.is_empty() {
            let avg_confidence: f32 = state.confidences.iter().sum::<f32>() / state.confidences.len() as f32;
            details.push(format!("{:.1}% avg conf", avg_confidence * 100.0));
        }

        let details_str = if details.is_empty() {
            String::new()
        } else {
            format!(" ({})", details.join(", "))
        };

        println!(
            "  {} {}{} \t{}",
            action_colored,
            state.resolved_path.display().to_string().cyan(),
            details_str.dimmed(),
            stats
        );
    }

    if !auto_apply && !ask_user("\nApply all changes in this revision?") {
        println!("  {} Revision aborted by user.\n", "⚠".yellow());
        return false;
    }

    let mut success_all = true;
    for state in valid_states {
        for hash in &state.hashes {
            cache.insert(*hash);
        }

        if dry_run {
            println!(
                "  {} [Dry Run] Would have applied action '{}' to {}\n",
                "ℹ".cyan(),
                state.action_char,
                state.resolved_path.display()
            );
            for hash in &state.hashes {
                cache.remove(hash);
            }
            continue;
        }

        match state.action_char {
            'A' | 'M' => {
                if let Some(parent) = state.resolved_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            error!("Failed to create directory structure {:?}: {}", parent, e);
                            eprintln!(
                                "  {} Failed to create folders for {}: {}",
                                "✖ Error:".red().bold(),
                                state.resolved_path.display(),
                                e
                            );
                            success_all = false;
                            continue;
                        }
                    }
                }

                if let Err(e) = fs::write(&state.resolved_path, &state.current_content) {
                    error!("Write error on path {:?}: {}", state.resolved_path, e);
                    eprintln!(
                        "  {} Failed to write to {}: {}",
                        "✖ Error:".red().bold(),
                        state.resolved_path.display(),
                        e
                    );
                    success_all = false;
                } else {
                    info!("Wrote file changes to {:?}", state.resolved_path);
                    println!(
                        "  {} Applied changes to {}",
                        "★".green().bold(),
                        state.resolved_path.display().to_string().green()
                    );
                }
            }
            'D' => {
                if let Err(e) = fs::remove_file(&state.resolved_path) {
                    error!("Delete error on path {:?}: {}", state.resolved_path, e);
                    eprintln!(
                        "  {} Failed to delete file {}: {}",
                        "✖ Error:".red().bold(),
                        state.resolved_path.display(),
                        e
                    );
                    success_all = false;
                } else {
                    info!("Deleted file {:?}", state.resolved_path);
                    println!(
                        "  {} Deleted {}",
                        "★".green().bold(),
                        state.resolved_path.display().to_string().green()
                    );
                }
            }
            _ => {}
        }
    }

    println!();
    success_all
}

pub fn find_existing_file(target: &str) -> Option<PathBuf> {
    debug!("Searching for existing file path: {}", target);
    let target_path = Path::new(target);
    if target_path.exists() && target_path.is_file() {
        return Some(target_path.to_path_buf());
    }

    let target_name = target_path.file_name()?;
    let mut candidates = Vec::new();
    for result in WalkBuilder::new(".").hidden(true).build() {
        if let Ok(entry) = result {
            if entry.file_type().map_or(false, |ft| ft.is_file()) {
                let path = entry.path();
                if path.file_name() == Some(target_name) {
                    if path.ends_with(target_path) {
                        return Some(path.to_path_buf());
                    }
                    candidates.push(path.to_path_buf());
                }
            }
        }
    }
    let selected = candidates.into_iter().next();
    debug!("Resolution for existing file returned: {:?}", selected);
    selected
}

pub fn find_best_target_file(
    target: &str,
    pattern_lines: &[LineInfo],
) -> Option<(PathBuf, Range<usize>, f32, Vec<String>)> {
    debug!("Fuzzy scanning candidate files matching: {}", target);
    let target_path = Path::new(target);

    let mut candidates = Vec::new();
    if target_path.exists() && target_path.is_file() {
        candidates.push(target_path.to_path_buf());
    } else if let Some(target_name) = target_path.file_name() {
        for result in WalkBuilder::new(".").hidden(true).build() {
            if let Ok(entry) = result {
                if entry.file_type().map_or(false, |ft| ft.is_file()) {
                    let path = entry.path();
                    if path.file_name() == Some(target_name) && path.ends_with(target_path) {
                        candidates.push(path.to_path_buf());
                    }
                }
            }
        }
    }

    let result = candidates
        .into_par_iter()
        .filter_map(|path| {
            if let Ok(content) = fs::read_to_string(&path) {
                let file_lines = to_line_info(&content);
                if let Some((byte_range, confidence, elisions)) =
                    fuzzy_find_block(&content, &file_lines, pattern_lines)
                {
                    trace!(
                        "File: {:?} scored {:.2}% match confidence",
                        path,
                        confidence * 100.0
                    );
                    return Some((path, byte_range, confidence, elisions));
                }
            }
            None
        })
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    debug!("Best fuzzy match candidate resolved: {:?}", result);
    result
}

// ------------------------------------------
// Elision Helpers
// ------------------------------------------

pub fn is_elision_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed == "// ..."
        || trimmed == "# ..."
        || trimmed == "/* ... */"
        || trimmed == "<!-- ... -->"
        || trimmed == "..."
        || trimmed == "//..."
        || trimmed == "#..."
}

// ------------------------------------------
// Optimized Levenshtein & DP Wildcard Logic
// ------------------------------------------

pub fn normalized_levenshtein(a_bytes: &[u8], b_bytes: &[u8], dp: &mut Vec<usize>) -> f32 {
    let a_len = a_bytes.len();
    let b_len = b_bytes.len();

    if a_len == 0 && b_len == 0 {
        return 1.0;
    }
    if a_len == 0 || b_len == 0 {
        return 0.0;
    }

    if dp.len() < b_len + 1 {
        dp.resize(b_len + 1, 0);
    }
    for j in 0..=b_len {
        dp[j] = j;
    }

    for i in 1..=a_len {
        let mut prev_diag = dp[0];
        dp[0] = i;
        for j in 1..=b_len {
            let temp = dp[j];
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            dp[j] = (dp[j] + 1).min(dp[j - 1] + 1).min(prev_diag + cost);
            prev_diag = temp;
        }
    }

    let max_len = a_len.max(b_len) as f32;
    let dist = dp[b_len] as f32;
    1.0 - (dist / max_len)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Move {
    Diag,
    Up,
    Left,
    None,
}

pub fn fuzzy_find_block(
    file_text: &str,
    file_lines: &[LineInfo],
    pattern_lines: &[LineInfo],
) -> Option<(Range<usize>, f32, Vec<String>)> {
    let m = pattern_lines.len();
    let n = file_lines.len();
    if m == 0 || n == 0 {
        return None;
    }

    let cols = n + 1;
    let mut dp = vec![0.0_f32; (m + 1) * cols];
    let mut moves = vec![Move::None; (m + 1) * cols];
    let gap_penalty = 0.5_f32;

    for j in 0..=n {
        dp[j] = 0.0;
    }
    for i in 1..=m {
        dp[i * cols] = -(i as f32) * gap_penalty;
        moves[i * cols] = Move::Up;
    }

    let mut lev_dp = Vec::with_capacity(128);

    for i in 1..=m {
        let p_text = pattern_lines[i - 1].text.trim();
        let p_is_elision = is_elision_line(p_text);

        for j in 1..=n {
            let f_text = file_lines[j - 1].text.trim();

            let match_score = if p_is_elision {
                1.0
            } else if p_text == f_text {
                1.0
            } else {
                let sim = normalized_levenshtein(p_text.as_bytes(), f_text.as_bytes(), &mut lev_dp);
                sim * 2.0 - 1.0
            };

            let diag = dp[(i - 1) * cols + (j - 1)] + match_score;
            let up = dp[(i - 1) * cols + j] - gap_penalty;

            let left_penalty = if p_is_elision { 0.0 } else { gap_penalty };
            let left = dp[i * cols + (j - 1)] - left_penalty;

            if diag >= up && diag >= left {
                dp[i * cols + j] = diag;
                moves[i * cols + j] = Move::Diag;
            } else if up >= diag && up >= left {
                dp[i * cols + j] = up;
                moves[i * cols + j] = Move::Up;
            } else {
                dp[i * cols + j] = left;
                moves[i * cols + j] = Move::Left;
            }
        }
    }

    let mut best_j = 1;
    let mut max_score = dp[m * cols + 1];
    for j in 2..=n {
        if dp[m * cols + j] > max_score {
            max_score = dp[m * cols + j];
            best_j = j;
        }
    }

    let mut el_matches: Vec<Vec<usize>> = vec![Vec::new(); m];
    let mut i = m;
    let mut j = best_j;
    let end_idx = j - 1;

    while i > 0 {
        let mv = moves[i * cols + j];
        let p_idx = i - 1;
        let f_idx = j - 1;
        let p_is_elision = is_elision_line(pattern_lines[p_idx].text);

        match mv {
            Move::Diag => {
                if p_is_elision {
                    el_matches[p_idx].push(f_idx);
                }
                i -= 1;
                j -= 1;
            }
            Move::Left => {
                if p_is_elision {
                    el_matches[p_idx].push(f_idx);
                }
                j -= 1;
            }
            Move::Up => {
                i -= 1;
            }
            Move::None => break,
        }
    }

    let start_idx = j;
    if start_idx <= end_idx && end_idx < n {
        let mut el_contents = Vec::new();
        for p_idx in 0..m {
            if is_elision_line(pattern_lines[p_idx].text) {
                let mut matches = el_matches[p_idx].clone();
                matches.reverse();

                let mut matched_text = String::new();
                if let Some(&first) = matches.first() {
                    let last = *matches.last().unwrap();
                    matched_text = file_text
                        [file_lines[first].byte_start..file_lines[last].byte_end]
                        .to_string();
                }
                el_contents.push(matched_text);
            }
        }

        let confidence = (max_score / m as f32).clamp(0.0, 1.0);
        Some((
            file_lines[start_idx].byte_start..file_lines[end_idx].byte_end,
            confidence,
            el_contents,
        ))
    } else {
        None
    }
}

// ------------------------------------------
// Utility Functions
// ------------------------------------------

pub fn print_diff(old: &str, new: &str, path: &Path) {
    let diff = TextDiff::from_lines(old, new);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&path.to_string_lossy(), &path.to_string_lossy())
        .to_string();
    if unified.is_empty() {
        return println!("  {}", "No changes detected.".yellow());
    }

    for line in unified.lines() {
        if line.starts_with("---") || line.starts_with("+++") {
            println!("    {}", line.bold());
        } else if line.starts_with("@@") {
            println!("    {}", line.cyan());
        } else if line.starts_with('+') {
            println!("    {}", line.green());
        } else if line.starts_with('-') {
            println!("    {}", line.red());
        } else {
            println!("    {}", line.dimmed());
        }
    }
}

pub fn ensure_trailing_newline(mut s: String) -> String {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn trim_newlines(s: &str) -> String {
    let mut start = 0;
    while start < s.len() && (s.as_bytes()[start] == b'\n' || s.as_bytes()[start] == b'\r') {
        start += 1;
    }
    let mut end = s.len();
    while end > start && (s.as_bytes()[end - 1] == b'\n' || s.as_bytes()[end - 1] == b'\r') {
        end -= 1;
    }
    s[start..end].to_string()
}

pub fn parse_instructions(input: &str) -> Vec<Instruction> {
    debug!("Parsing instructions out of payload...");
    let mut instructions = Vec::new();
    let mut current_pos = 0;

    loop {
        let update_pos = input[current_pos..].find("<update>");
        let create_pos = input[current_pos..].find("<create>");
        let delete_pos = input[current_pos..].find("<delete>");

        let mut next_tag = None;
        let mut min_pos = usize::MAX;

        if let Some(pos) = update_pos {
            if pos < min_pos {
                min_pos = pos;
                next_tag = Some(("update", "<update>", "</update>"));
            }
        }
        if let Some(pos) = create_pos {
            if pos < min_pos {
                min_pos = pos;
                next_tag = Some(("create", "<create>", "</create>"));
            }
        }
        if let Some(pos) = delete_pos {
            if pos < min_pos {
                min_pos = pos;
                next_tag = Some(("delete", "<delete>", "</delete>"));
            }
        }

        if let Some((tag_name, start_tag, end_tag)) = next_tag {
            let absolute_start = current_pos + min_pos;
            if let Some(end_idx) = input[absolute_start..].find(end_tag) {
                let absolute_end = absolute_start + end_idx + end_tag.len();
                let block = &input[absolute_start..absolute_end];

                match tag_name {
                    "update" => {
                        if let Some(inner) = extract_tag_content(block, "update") {
                            if let Some(f) = extract_tag_content(&inner, "file") {
                                let raw_body = remove_file_tag(&inner);
                                let mut cleaned_body = raw_body;
                                let trimmed = cleaned_body.trim();
                                if trimmed.starts_with("<![CDATA[") && trimmed.ends_with("]]>") {
                                    cleaned_body = trimmed[9..trimmed.len() - 3].to_string();
                                }
                                cleaned_body = trim_newlines(&cleaned_body);

                                let mut prefix_lines = Vec::new();
                                let mut replacement_lines = Vec::new();
                                let mut suffix_lines = Vec::new();
                                let mut state = 0;

                                for line in cleaned_body.split_inclusive('\n') {
                                    if line.trim() == "@@@" {
                                        state += 1;
                                        continue;
                                    }
                                    match state {
                                        0 => prefix_lines.push(line),
                                        1 => replacement_lines.push(line),
                                        2 => suffix_lines.push(line),
                                        _ => {}
                                    }
                                }

                                if state >= 2 {
                                    let prefix = ensure_trailing_newline(prefix_lines.concat());
                                    let replacement = ensure_trailing_newline(replacement_lines.concat());
                                    let suffix = ensure_trailing_newline(suffix_lines.concat());

                                    instructions.push(Instruction::Update {
                                        file_target: f.trim().to_string(),
                                        prefix,
                                        replacement,
                                        suffix,
                                    });
                                } else {
                                    warn!(
                                        "Update block for {} did not contain exactly two '@@@' separators",
                                        f.trim()
                                    );
                                }
                            }
                        }
                    }
                    "create" => {
                        if let (Some(f), Some(c)) = (
                            extract_tag_content(block, "file"),
                            extract_tag_content(block, "content"),
                        ) {
                            instructions.push(Instruction::Create {
                                file_target: f.trim().to_string(),
                                content: c,
                            });
                        }
                    }
                    "delete" => {
                        if let Some(f) = extract_tag_content(block, "file") {
                            instructions.push(Instruction::Delete {
                                file_target: f.trim().to_string(),
                            });
                        }
                    }
                    _ => {}
                }
                current_pos = absolute_end;
            } else {
                current_pos = absolute_start + start_tag.len();
            }
        } else {
            break;
        }
    }
    debug!("Successfully parsed {} instructions", instructions.len());
    instructions
}

pub fn extract_tag_content(input: &str, tag: &str) -> Option<String> {
    let start = input.find(&format!("<{}>", tag))? + tag.len() + 2;
    let end = start + input[start..].find(&format!("</{}>", tag))?;
    let mut content = &input[start..end];
    if content.starts_with("<![CDATA[") && content.ends_with("]]>") {
        content = &content[9..content.len() - 3];
    }
    Some(content.to_string())
}

pub fn remove_file_tag(input: &str) -> String {
    if let Some(start_idx) = input.find("<file>") {
        if let Some(end_idx) = input[start_idx..].find("</file>") {
            let mut result = input[..start_idx].to_string();
            result.push_str(&input[start_idx + end_idx + 7..]);
            return result;
        }
    }
    input.to_string()
}

pub fn ask_user(msg: &str) -> bool {
    print!("{} [y/N]: ", msg.bold());
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap_or_default();
    input.trim().eq_ignore_ascii_case("y")
}

#[derive(Debug)]
pub struct LineInfo<'a> {
    pub text: &'a str,
    pub byte_start: usize,
    pub byte_end: usize,
}

pub fn to_line_info(text: &str) -> Vec<LineInfo> {
    let mut lines = Vec::new();
    let mut current_byte = 0;
    for line in text.split_inclusive('\n') {
        let byte_len = line.len();
        lines.push(LineInfo {
            text: line,
            byte_start: current_byte,
            byte_end: current_byte + byte_len,
        });
        current_byte += byte_len;
    }
    lines
}
