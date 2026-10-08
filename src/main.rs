use std::{
    cmp::Ordering,
    fmt, io,
    io::{Read, Write},
    path::PathBuf,
    process::ExitCode,
    str::FromStr,
};

use clap::Parser;
use feruca::{Collator, Tailoring};

/// Sort a todo.txt file. Multi-key, SQL-style: the first --sort key decides,
/// later keys break ties. Sorting is stable — lines with equal keys keep
/// their input order. Lines are also normalized (canonical prefix order,
/// tags moved to the end) — see --no-format-lines.
#[derive(Parser)]
#[command(
    name = "todofmt",
    version,
    about,
    after_help = "examples:
  todofmt todo.txt                          open tasks first (A-Z), done tasks sink to the bottom
  todofmt --sort completed:desc --sort timestamp  done tasks on top, newest first
  todofmt -s prio,text -r < todo.txt > sorted.txt
  todofmt --no-format-lines -i todo.txt           only reorder lines, leave line contents untouched
  todofmt -c todo.txt                       exit 1 if not in target form yet (pre-commit hooks)"
)]
struct Cli {
    /// Sort key(s): KEY[:DIR] with DIR = asc|desc (default asc).
    /// Keys: completed, timestamp, text, priority, due.
    /// Repeatable and comma-separable; order = priority.
    #[arg(
        short = 's',
        long = "sort",
        value_name = "KEY[:DIR]",
        value_delimiter = ','
    )]
    sort: Vec<SortKey>,

    /// Reverse the final order
    #[arg(short = 'r', long)]
    reverse: bool,

    /// Only reorder lines; leave each line's contents untouched
    /// (disables the default normalization, incl. done-prio stripping)
    #[arg(long)]
    no_format_lines: bool,

    /// Exit 0 if the input is already in target form, 1 otherwise
    /// (silent on success). For pre-commit hooks / CI.
    #[arg(short = 'c', long, conflicts_with = "in_place")]
    check: bool,

    /// Rewrite FILE in place instead of printing to stdout
    #[arg(
        short = 'i',
        long = "in-place",
        visible_short_alias = 'w',
        visible_alias = "write",
        requires = "file"
    )]
    in_place: bool,

    /// todo.txt file (reads stdin when omitted)
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Completed,
    Timestamp,
    Text,
    Priority,
    Due,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct SortKey {
    field: Field,
    dir: Dir,
}

impl FromStr for SortKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, dir) = match s.rsplit_once(':') {
            Some((n, d)) => (
                n,
                match d.to_ascii_lowercase().as_str() {
                    "asc" => Dir::Asc,
                    "desc" => Dir::Desc,
                    _ => return Err(format!("invalid direction '{d}' (use asc or desc)")),
                },
            ),
            None => (s, Dir::Asc),
        };
        let field = match name.trim().to_ascii_lowercase().as_str() {
            "completed" | "done" | "checked" => Field::Completed,
            "timestamp" | "timestamps" | "date" | "time" => Field::Timestamp,
            "text" | "alphabetical" | "alphabetically" | "alpha" | "name" => Field::Text,
            "priority" | "prio" => Field::Priority,
            "due" => Field::Due,
            other => {
                return Err(format!(
                    "unknown sort key '{other}' (completed, timestamp, text, priority, due)"
                ))
            }
        };
        Ok(SortKey { field, dir })
    }
}

impl fmt::Display for SortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = match self.field {
            Field::Completed => "completed",
            Field::Timestamp => "timestamp",
            Field::Text => "text",
            Field::Priority => "priority",
            Field::Due => "due",
        };
        match self.dir {
            Dir::Asc => write!(f, "{field}"),
            Dir::Desc => write!(f, "{field}:desc"),
        }
    }
}

#[derive(Debug, Default)]
struct Task {
    raw: String,
    completed: bool,
    completion: Option<String>,
    created: Option<String>,
    priority: Option<char>,
    due: Option<String>,
    text: String,
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => true,
            _ => c.is_ascii_digit(),
        })
}

fn priority_of(s: &str) -> Option<char> {
    let b = s.as_bytes();
    match b.len() == 3 && b[0] == b'(' && b[2] == b')' && b[1].is_ascii_uppercase() {
        true => Some(b[1] as char),
        false => None,
    }
}

fn parse(line: &str) -> Task {
    let mut task = Task {
        raw: line.to_string(),
        ..Task::default()
    };
    let mut rest = line;
    let mut first = true;

    loop {
        let trimmed = rest.trim_start();
        let tok = trimmed.split_whitespace().next().unwrap_or("");
        if tok.is_empty() {
            break;
        }

        if first && tok == "x" {
            task.completed = true;
            rest = &trimmed[tok.len()..];
        } else if task.priority.is_none() && priority_of(tok).is_some() {
            task.priority = priority_of(tok);
            rest = &trimmed[tok.len()..];
        } else if is_date(tok) && task.completion.is_none() && task.completed {
            task.completion = Some(tok.to_string());
            rest = &trimmed[tok.len()..];
        } else if is_date(tok)
            && task.created.is_none()
            && (task.completion.is_some() || !task.completed)
        {
            task.created = Some(tok.to_string());
            rest = &trimmed[tok.len()..];
        } else {
            break;
        }
        first = false;
    }

    task.text = rest.trim().to_string();
    task.due = rest
        .split_whitespace()
        .find(|t| t.to_ascii_lowercase().starts_with("due:"))
        .map(|t| t[4..].to_string());
    task
}

fn cmp_opt<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn cmp_field(a: &Task, b: &Task, field: Field, collator: &mut Collator) -> Ordering {
    match field {
        // asc: open tasks (false) before completed (true)
        Field::Completed => a.completed.cmp(&b.completed),
        // completion date on done lines, created date otherwise
        Field::Timestamp => cmp_opt(
            a.completion.as_ref().or(a.created.as_ref()),
            b.completion.as_ref().or(b.created.as_ref()),
        ),
        Field::Text => collator.collate(&a.text, &b.text),
        Field::Priority => cmp_opt(a.priority, b.priority),
        Field::Due => cmp_opt(a.due.clone(), b.due.clone()),
    }
}

fn sort_tasks(tasks: &mut [Task], keys: &[SortKey], collator: &mut Collator) {
    tasks.sort_by(|a, b| {
        for key in keys {
            let mut ord = cmp_field(a, b, key.field, collator);
            if key.dir == Dir::Desc {
                ord = ord.reverse();
            }
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
}

const DEFAULT_KEYS: [SortKey; 2] = [
    SortKey { field: Field::Completed, dir: Dir::Asc },
    SortKey { field: Field::Text, dir: Dir::Asc },
];

/// key:value tokens that are recognized as todo.txt extension tags and
/// moved to the end of the line during normalization. Deliberately small —
/// anything else (URLs, times like 12:30) stays in the text.
const SPECIAL_KEYS: [&str; 6] = ["due:", "t:", "thresh:", "pri:", "h:", "rec:"];

fn split_tags(text: &str) -> (Vec<&str>, Vec<String>, Vec<String>, Vec<String>) {
    let mut core = Vec::new();
    let (mut projects, mut contexts, mut kvs) = (Vec::new(), Vec::new(), Vec::new());
    for tok in text.split_whitespace() {
        let lower = tok.to_ascii_lowercase();
        if tok.len() > 1 && tok.starts_with('+') {
            projects.push(tok.to_string());
        } else if tok.len() > 1 && tok.starts_with('@') {
            contexts.push(tok.to_string());
        } else if SPECIAL_KEYS.iter().any(|k| lower.starts_with(k)) {
            kvs.push(tok.to_string());
        } else {
            core.push(tok);
        }
    }
    projects.sort_by_key(|t| t.to_lowercase());
    contexts.sort_by_key(|t| t.to_lowercase());
    kvs.sort_by_key(|t| t.to_lowercase());
    (core, projects, contexts, kvs)
}

impl Task {
    /// Canonical line form (todo.sh de-facto order):
    /// x, done-date, (prio), created-date, text, +projects, @contexts, key:values.
    /// Completed tasks lose their priority (spec hygiene — keeps the active
    /// priority namespace clean). The task text itself is preserved verbatim —
    /// only structural tokens and tags are moved/sorted.
    fn canonical(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.completed {
            parts.push("x".to_string());
        }
        if let Some(d) = &self.completion {
            parts.push(d.clone());
        }
        if let Some(p) = self.priority.filter(|_| !self.completed) {
            parts.push(format!("({p})"));
        }
        if let Some(d) = &self.created {
            parts.push(d.clone());
        }
        let (core, projects, contexts, kvs) = split_tags(&self.text);
        let mut text: Vec<String> = core.into_iter().map(String::from).collect();
        text.extend(projects);
        text.extend(contexts);
        text.extend(kvs);
        if !text.is_empty() {
            parts.push(text.join(" "));
        }
        parts.join(" ")
    }
}

fn apply(input: &str, keys: &[SortKey], reverse: bool, no_format_lines: bool) -> String {
    let mut tasks: Vec<Task> = input
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse)
        .collect();

    let mut collator = Collator::new(Tailoring::default(), true, true);
    sort_tasks(&mut tasks, keys, &mut collator);

    if reverse {
        tasks.reverse();
    }

    if !no_format_lines {
        for t in &mut tasks {
            t.raw = t.canonical();
        }
    }

    let mut out = tasks
        .into_iter()
        .map(|t| t.raw)
        .collect::<Vec<_>>()
        .join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn run(cli: &Cli) -> Result<(String, String), String> {
    let input = match &cli.file {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        None => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("cannot read stdin: {e}"))?;
            buf
        }
    };

    let keys: &[SortKey] = if cli.sort.is_empty() {
        &DEFAULT_KEYS
    } else {
        &cli.sort
    };
    let output = apply(&input, keys, cli.reverse, cli.no_format_lines);
    Ok((input, output))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok((input, output)) => {
            if cli.check {
                if input.trim_end() == output.trim_end() {
                    ExitCode::SUCCESS
                } else {
                    let name = cli
                        .file
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(stdin)".to_string());
                    eprintln!("todofmt: {name} is not in target form");
                    ExitCode::FAILURE
                }
            } else if cli.in_place {
                let path = cli.file.as_ref().unwrap();
                match std::fs::write(path, &output) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("todofmt: cannot write {}: {e}", path.display());
                        ExitCode::FAILURE
                    }
                }
            } else {
                match io::stdout().write_all(output.as_bytes()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("todofmt: cannot write stdout: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("todofmt: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(input: &str, keys: &[SortKey]) -> Vec<String> {
        let keys: &[SortKey] = if keys.is_empty() { &DEFAULT_KEYS } else { keys };
        let mut tasks: Vec<Task> = input.lines().map(parse).collect();
        let mut collator = Collator::new(Tailoring::default(), true, true);
        sort_tasks(&mut tasks, keys, &mut collator);
        tasks.into_iter().map(|t| t.raw).collect()
    }

    fn key(spec: &str) -> SortKey {
        spec.parse().unwrap()
    }

    #[test]
    fn parses_done_line_with_dates_and_priority() {
        let t = parse("x 2026-10-03 (A) 2026-10-02 write docs +proj @ctx");
        assert!(t.completed);
        assert_eq!(t.completion.as_deref(), Some("2026-10-03"));
        assert_eq!(t.created.as_deref(), Some("2026-10-02"));
        assert_eq!(t.priority, Some('A'));
        assert_eq!(t.text, "write docs +proj @ctx");
    }

    #[test]
    fn parses_open_line_with_created_date() {
        let t = parse("2026-10-02 buy rice +einkauf");
        assert!(!t.completed);
        assert_eq!(t.created.as_deref(), Some("2026-10-02"));
        assert_eq!(t.text, "buy rice +einkauf");
    }

    #[test]
    fn parses_due_date_anywhere() {
        let t = parse("file taxes due:2026-11-01");
        assert_eq!(t.due.as_deref(), Some("2026-11-01"));
        assert_eq!(t.text, "file taxes due:2026-11-01");
    }

    #[test]
    fn default_puts_open_first_then_done() {
        let input = "x 2026-10-08 old task\nzebra\napple\nx 2026-10-07 alpha done";
        assert_eq!(
            sorted(input, &[]),
            vec!["apple", "zebra", "x 2026-10-07 alpha done", "x 2026-10-08 old task"]
        );
    }

    #[test]
    fn completed_desc_puts_done_on_top() {
        let input = "apple\nx 2026-10-08 done task";
        let keys = &[key("completed:desc"), key("text")];
        assert_eq!(sorted(input, keys), vec!["x 2026-10-08 done task", "apple"]);
    }

    #[test]
    fn timestamps_use_completion_else_created() {
        let input = "2026-10-05 open old\nx 2026-10-01 done\nx 2026-10-09 done new\n2026-10-03 open new";
        let keys = &[key("timestamp")];
        assert_eq!(
            sorted(input, keys),
            vec![
                "x 2026-10-01 done",
                "2026-10-03 open new",
                "2026-10-05 open old",
                "x 2026-10-09 done new",
            ]
        );
    }

    #[test]
    fn order_of_keys_matters_like_sql() {
        let input = "2026-10-01 b\n(A) a\n2026-10-02 a\n(B) c";
        // priority first, then timestamp (tiebreaker within equal priority)
        let keys = &[key("priority"), key("timestamp")];
        assert_eq!(
            sorted(input, keys),
            vec!["(A) a", "(B) c", "2026-10-01 b", "2026-10-02 a"]
        );
        // timestamp first, then priority — undated tasks sink to the bottom
        let keys = &[key("timestamp"), key("priority")];
        assert_eq!(
            sorted(input, keys),
            vec!["2026-10-01 b", "2026-10-02 a", "(A) a", "(B) c"]
        );
    }

    #[test]
    fn reverse_flips_final_order() {
        let input = "apple\nbanana";
        let keys = &[key("text")];
        assert_eq!(sorted(input, keys), vec!["apple", "banana"]);
        let mut tasks: Vec<Task> = input.lines().map(parse).collect();
        let mut collator = Collator::new(Tailoring::default(), true, true);
        sort_tasks(&mut tasks, keys, &mut collator);
        tasks.reverse();
        assert_eq!(
            tasks.into_iter().map(|t| t.raw).collect::<Vec<_>>(),
            vec!["banana", "apple"]
        );
    }

    #[test]
    fn sort_is_stable() {
        let input = "2026-10-01 first\n2026-10-01 second\n2026-10-01 third";
        let keys = &[key("timestamp")];
        assert_eq!(
            sorted(input, keys),
            vec!["2026-10-01 first", "2026-10-01 second", "2026-10-01 third"]
        );
    }

    #[test]
    fn collation_handles_umlauts() {
        let input = "Übergeben\napfel\nÄrzte\nZebra\nbrot";
        let keys = &[key("text")];
        assert_eq!(
            sorted(input, keys),
            vec!["apfel", "Ärzte", "brot", "Übergeben", "Zebra"]
        );
    }

    #[test]
    fn key_parsing_accepts_aliases_and_directions() {
        assert_eq!(key("done"), key("completed"));
        assert_eq!(key("alphabetically"), key("text"));
        assert_eq!(key("timestamps"), key("date"));
        assert_eq!(key("prio"), key("priority"));
        assert_eq!(
            key("timestamp:DESC"),
            SortKey { field: Field::Timestamp, dir: Dir::Desc }
        );
        assert!("timestamp:down".parse::<SortKey>().is_err());
        assert!("nope".parse::<SortKey>().is_err());
    }

    #[test]
    fn dates_sort_chronologically_as_strings() {
        assert!("2026-09-30" < "2026-10-01");
        assert!("2025-12-31" < "2026-01-01");
    }

    fn formatted(input: &str) -> String {
        apply(input, &DEFAULT_KEYS, false, false)
    }

    #[test]
    fn normalization_is_on_by_default() {
        assert_eq!(formatted("buy @b +z milk +a @a\n"), "buy milk +a +z @a @b\n");
    }

    #[test]
    fn canonical_prefix_order() {
        // legal prio position (right after x): done tasks lose their priority
        assert_eq!(
            formatted("x (A) 2026-10-08 done task +z @web\n"),
            "x 2026-10-08 done task +z @web\n"
        );
        // mid-text "(A)" is treated as literal text, not a priority (conservative)
        assert_eq!(
            formatted("x 2026-10-08 done @web (A) +z\n"),
            "x 2026-10-08 done (A) +z @web\n"
        );
        assert_eq!(formatted("(B) 2026-10-01 open +z\n"), "(B) 2026-10-01 open +z\n");
    }

    #[test]
    fn no_format_lines_leaves_lines_untouched() {
        let out = apply("buy @b +z milk\n", &DEFAULT_KEYS, false, true);
        assert_eq!(out, "buy @b +z milk\n");
    }

    #[test]
    fn normalization_strips_done_prio_by_default() {
        let out = formatted("(A) open one\nx 2026-10-08 (A) done task\n");
        assert_eq!(out, "(A) open one\nx 2026-10-08 done task\n");
    }

    #[test]
    fn no_format_lines_keeps_bytes_including_done_prio() {
        let out = apply(
            "(A) open @b one\nx 2026-10-08 (A) done @a\n",
            &DEFAULT_KEYS,
            false,
            true,
        );
        assert_eq!(out, "(A) open @b one\nx 2026-10-08 (A) done @a\n");
    }

    #[test]
    fn urls_and_times_stay_in_text() {
        let line = "check https://example.com/a+b at 12:30 due:2026-11-01\n";
        assert_eq!(formatted(line), line);
    }

    #[test]
    fn check_mode_compares_target_form() {
        // formatted input → unchanged output
        let clean = formatted("(A) open one\nx 2026-10-08 done task\n");
        assert_eq!(clean, "(A) open one\nx 2026-10-08 done task\n");
        // messy input → output differs from input
        let messy = "(A) open @b one\nx (A) 2026-10-08 done task\n";
        assert_ne!(formatted(messy), messy);
    }
}
