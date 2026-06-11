//! Replay a captured bob `--output-format stream-json` file through the fixed
//! `BobStreamParser` and print what the host would render — to verify
//! echo-suppression + thinking-routing on REAL bob output (not synthetic).
//!
//! cargo run --example replay_bob --all-features -- /path/to/out.jsonl

use std::io::BufRead;

use harness::bob::BobStreamParser;

fn main() {
    let path = std::env::args().nth(1).expect("usage: replay_bob <out.jsonl>");
    let file = std::fs::File::open(&path).expect("open file");
    let mut parser = BobStreamParser::default();
    let (mut text, mut thinking, mut tools) = (String::new(), String::new(), Vec::new());
    for line in std::io::BufReader::new(file).lines() {
        let parsed = parser.parse_line(&line.unwrap());
        if let Some(t) = parsed.text {
            text.push_str(&t);
        }
        if let Some(t) = parsed.thinking {
            thinking.push_str(&t);
        }
        if let Some(ts) = parsed.tool_start {
            tools.push(format!("ToolStart({})", ts.name));
        }
        if parsed.tool_end.is_some() {
            tools.push("ToolEnd".to_owned());
        }
    }
    println!("=== VISIBLE TEXT (host renders this as the message) ===\n{text}\n");
    println!("contains '[using tool' ? {}", text.contains("[using tool"));
    println!("contains '<thinking>'  ? {}", text.contains("<thinking>"));
    println!("\n=== THINKING (routed to its own section) ===\n{}", thinking.trim());
    println!("\n=== TOOL EVENTS ===");
    for t in &tools {
        println!("  {t}");
    }
}
