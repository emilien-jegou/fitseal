# Fitseal 🦭

**Fitseal** (**FIT**ting **SE**quence **AL**ignment) is a fuzzy patcher daemon written in Rust for AI answers.

## Features

- **Fuzzy Block Matching**: Uses dynamic programming to slide the match pattern across your target file, resilient to drift, typos, changed variable names, or newly inserted comments.
- **Multiple Instruction Types**: Fully supports XML-based instructions to modify (`<update>`), generate (`<create>`), and remove (`<delete>`) files in your workspace.
- **Clipboard Daemon**: Run Fitseal in the background. As soon as you copy any instruction blocks (e.g., from ChatGPT or Claude), Fitseal instantly parses them. (Fully supports Wayland and X11).
- **Consolidated Transaction Prompts**: If your clipboard text contains multiple instruction blocks, Fitseal groups them together and prompts you only once to apply the unified revision instead of prompting you for each file sequentially.
- **Blazing Fast**: Uses `ignore` to instantly skip `.git`, `target`, and `node_modules` directories, and `rayon` to evaluate potential target files in parallel. 
- **Intelligent File Resolution**: If an instruction provides a non-absolute path (e.g. `main.rs`), Fitseal searches your project, scores all matching files, and targets the one with the highest confidence.
- **Git-Style Diffs**: Displays a unified diff (`+`/`-`) and structured change stats of the revision before asking you to confirm.
- **Smart Caching**: Remembers which patches you've already applied or discarded during a session, preventing annoying duplicate prompts.

## Usage

Watch the clipboard for instructions formatted as `<update>`, `<create>`, or `<delete>`. 

```bash
fitseal daemon
```

Whenever blocks are copied to the clipboard, Fitseal groups them into a revision summary and displays the comprehensive change plan:

```text
🦭 Fitseal daemon started. Watching clipboard for instruction blocks...

→ Processing instruction block(s)!

Proposed Revision Summary:
  D ./file.txt           -12
  M ./src/main.rs        +12 -4
  A ./src/other.rs       +24

Detailed Diffs:

--- ./src/main.rs ---
    @@ -45,3 +45,3 @@
    - fn handle_arrow() {
    -     // Old logic
    + fn handle_arrow(faster: bool) {
    +     // New logic
      }

Apply all changes in this revision? [y/N]: y
  ★ Deleted ./file.txt
  ★ Applied changes to ./src/main.rs
  ★ Applied changes to ./src/other.rs
```

To run non-interactively (auto-applies if confidence > 40%):
```bash
fitseal daemon --auto-apply
```

### Logging

Fitseal utilizes standard Rust `tracing` logs. To debug the engine or view candidates evaluated by the fuzzy matching pipeline, launch with the `RUST_LOG` environment variable set:

```bash
RUST_LOG=debug fitseal daemon
```

---

## License

MIT License. See `LICENSE` for details.
