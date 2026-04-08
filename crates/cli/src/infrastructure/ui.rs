use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

// ── Environment detection ─────────────────────────────────────────────────────

/// True when running inside a known CI/CD environment.
pub fn is_ci() -> bool {
    std::env::var("CI").is_ok()
        || std::env::var("GITHUB_ACTIONS").is_ok()
        || std::env::var("CONTINUOUS_INTEGRATION").is_ok()
        || std::env::var("BUILDKITE").is_ok()
        || std::env::var("CIRCLECI").is_ok()
        || std::env::var("TRAVIS").is_ok()
        || std::env::var("GITLAB_CI").is_ok()
}

/// True when stdout is an interactive terminal AND not in CI.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal;
    !is_ci() && std::io::stdout().is_terminal()
}

// ── Progress bar ──────────────────────────────────────────────────────────────

/// Create a block-fill progress bar:  →  Label  ████████████  100%
/// In CI / non-TTY mode, prints the label as plain text and returns a hidden bar.
pub fn progress_bar(label: impl Into<String>, total: u64) -> ProgressBar {
    let label = label.into();
    if !is_interactive() {
        println!("{label}");
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("  {prefix:.bold}  {bar:12.cyan/blue.dim}  {percent:>3}%")
            .unwrap()
            .progress_chars("█░"),
    );
    pb.set_prefix(format!("→ {label}"));
    pb
}

/// Finish a progress bar at 100% and print a green ✓ success line below it.
pub fn bar_done(pb: &ProgressBar, msg: impl AsRef<str>) {
    pb.finish();
    println!("  {}  {}", style("✓").green().bold(), msg.as_ref());
}

/// Abandon a progress bar (partial) and exit 1 with a red ✗ error line.
pub fn bar_fail(pb: &ProgressBar, msg: impl AsRef<str>) -> ! {
    pb.abandon();
    eprintln!(
        "  {}  {}",
        style("✗").red().bold(),
        style(msg.as_ref()).red()
    );
    std::process::exit(1);
}

// ── Spinner ───────────────────────────────────────────────────────────────────

/// Create an animated spinner for a long-running operation.
/// In CI / non-TTY mode, prints the message as plain text and returns a
/// hidden no-op bar so callers don't need special-casing.
pub fn spinner(msg: impl Into<String>) -> ProgressBar {
    let msg = msg.into();
    if !is_interactive() {
        println!("{msg}");
        return ProgressBar::hidden();
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.cyan}  {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// Finish spinner with a success message.
pub fn done(pb: &ProgressBar, msg: impl AsRef<str>) {
    pb.finish_and_clear();
    println!("  {}  {}", style("✓").green().bold(), msg.as_ref());
}

/// Finish spinner with an error message, then exit 1.
pub fn fail(pb: &ProgressBar, msg: impl AsRef<str>) -> ! {
    pb.finish_and_clear();
    eprintln!(
        "  {}  {}",
        style("✗").red().bold(),
        style(msg.as_ref()).red()
    );
    std::process::exit(1);
}

// ── One-liner output helpers ──────────────────────────────────────────────────

/// Print a green ✓ success line.
pub fn success(msg: impl AsRef<str>) {
    println!("  {}  {}", style("✓").green().bold(), msg.as_ref());
}

/// Print a red ✗ error line to stderr, then exit 1.
pub fn die(msg: impl AsRef<str>) -> ! {
    eprintln!(
        "  {}  {}",
        style("✗").red().bold(),
        style(msg.as_ref()).red()
    );
    std::process::exit(1);
}

/// Print a red ✗ error line to stderr (no exit).
pub fn error(msg: impl AsRef<str>) {
    eprintln!(
        "  {}  {}",
        style("✗").red().bold(),
        style(msg.as_ref()).red()
    );
}

/// Print a yellow ! warning line.
pub fn warn(msg: impl AsRef<str>) {
    eprintln!(
        "  {}  {}",
        style("!").yellow().bold(),
        style(msg.as_ref()).yellow()
    );
}

/// Print a dim info line.
pub fn info(msg: impl AsRef<str>) {
    println!("  {}  {}", style("·").dim(), msg.as_ref());
}

/// Print a dim hint (e.g. "Next steps:" suggestions).
pub fn hint(msg: impl AsRef<str>) {
    println!("  {}", style(msg.as_ref()).dim());
}

/// Print a blank line.
pub fn blank() {
    println!();
}

/// Print a bold cyan section header.
pub fn header(title: impl AsRef<str>) {
    println!();
    println!("  {}", style(title.as_ref()).bold().cyan());
    println!();
}

/// Print a key / value pair with aligned columns.
pub fn kv(key: impl AsRef<str>, val: impl AsRef<str>) {
    println!("  {:<20}{}", style(key.as_ref()).dim(), val.as_ref());
}

// ── Table ─────────────────────────────────────────────────────────────────────

/// A simple text table printed with box-drawing characters.
/// Columns widths are auto-calculated.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: Vec<&str>) -> Self {
        Self {
            headers: headers.into_iter().map(String::from).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cols: Vec<impl Into<String>>) {
        self.rows.push(cols.into_iter().map(Into::into).collect());
    }

    pub fn print(&self) {
        if self.rows.is_empty() {
            println!("  {}", style("(no results)").dim());
            return;
        }

        // Compute column widths
        let ncols = self.headers.len();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < ncols {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        let sep = |left: &str, mid: &str, right: &str, fill: &str| {
            let inner: Vec<String> = widths.iter().map(|&w| fill.repeat(w + 2)).collect();
            format!("  {}{}{}", left, inner.join(mid), right)
        };

        println!("{}", style(sep("┌", "┬", "┐", "─")).dim());

        // Header
        let header_row: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!(" {:<width$} ", h, width = widths[i]))
            .collect();
        println!(
            "  {}{}",
            style("│").dim(),
            header_row
                .iter()
                .map(|c| format!("{}{}", style(c).bold(), style("│").dim()))
                .collect::<Vec<_>>()
                .join("")
        );

        println!("{}", style(sep("├", "┼", "┤", "─")).dim());

        // Rows
        for row in &self.rows {
            let cells: Vec<String> = (0..ncols)
                .map(|i| {
                    let cell = row.get(i).map(String::as_str).unwrap_or("");
                    format!(" {:<width$} ", cell, width = widths[i])
                })
                .collect();
            println!(
                "  {}{}",
                style("│").dim(),
                cells
                    .iter()
                    .map(|c| format!("{}{}", c, style("│").dim()))
                    .collect::<Vec<_>>()
                    .join("")
            );
        }

        println!("{}", style(sep("└", "┴", "┘", "─")).dim());
    }
}

// ── Interactive prompts ───────────────────────────────────────────────────────

/// Prompt for a text value. In CI mode, returns the default.
pub fn input(prompt: &str, default: Option<&str>) -> String {
    if !is_interactive() {
        return default.unwrap_or("").to_string();
    }
    let mut builder = dialoguer::Input::<String>::new().with_prompt(prompt);
    if let Some(d) = default {
        builder = builder.default(d.to_string());
    }
    builder
        .interact_text()
        .unwrap_or_else(|_| default.unwrap_or("").to_string())
}

/// Prompt for a password (hidden input). In CI mode, reads NEXA_PASSWORD env var.
pub fn password(prompt: &str) -> String {
    if !is_interactive() {
        return std::env::var("NEXA_PASSWORD").unwrap_or_default();
    }
    dialoguer::Password::new()
        .with_prompt(prompt)
        .interact()
        .unwrap_or_default()
}

/// Ask a yes/no question. In CI mode, returns `default`.
pub fn confirm(prompt: &str, default: bool) -> bool {
    if !is_interactive() {
        return default;
    }
    dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(default)
        .interact()
        .unwrap_or(default)
}
