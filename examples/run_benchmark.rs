use anyhow::Result;
use issuemesh::config::{Config, Secrets};
use issuemesh::paths::Paths;
use issuemesh::search::orchestrator::SearchOrchestrator;
use issuemesh::store::Store;
use issuemesh::AppState;
use std::sync::Arc;

const QUESTIONS: &[&str] = &[
    "What is the current status of OPEN-4679, and what work remains?",
    "Find all GitHub PRs, Launchpad bugs and Jira issues directly related to OPEN-4679. Explain the relationships.",
    "What are the main unresolved BGP limitations affecting Sunbeam today?",
    "Is BFD support currently implemented for control-plane VIPs? Show the evidence.",
    "Which issues are blocking multi-site OpenStack deployments, and why?",
    "What engineering work has been completed for OVN BGP, and what is still open?",
    "Find issues related to tenant network advertisement that do not explicitly contain the words \"tenant network advertisement\".",
    "Which GitHub PRs claim to fix or implement Jira issues related to BGP?",
    "Are there Jira issues marked Done whose linked GitHub implementation is still open?",
    "Are there Launchpad bugs marked Fix Released whose related implementation appears incomplete elsewhere?",
    "What changed during the last 30 days regarding BGP support?",
    "What are the three most important unresolved issues for production readiness of Sunbeam? Explain why.",
    "Find everything related to OPEN-4047 up to two relationship hops away. Summarise only what is relevant to the issue.",
    "Does PF-6946 depend on, block, duplicate, or relate to other issues? Distinguish explicit links from inferred relationships.",
    "What evidence exists that a specific feature requested in Jira has actually landed in code?",
    "Which unresolved issues mention or affect both OpenStack and Kubernetes networking?",
    "What are the oldest still-open high-priority issues related to Sunbeam networking?",
    "Who is currently responsible for the main unresolved BGP issues, according to the trackers?",
    "Is there evidence in Jira, GitHub or Launchpad that IssueMesh itself supports SAP HANA deployment automation?",
    "Why are modern multi-site deployments driving new BGP requirements, based only on tracker evidence?",
    "Are there circular dependencies or cyclic links among Sunbeam networking issues, and how does graph traversal handle them?",
    "What is the latest status of OPEN-4679 after recent remote updates, and how does newer remote data affect local cache?",
    "If a tracker is disabled or temporarily unavailable, what can be determined about BGP VIP support without inventing data?",
    "How does IssueMesh handle tracker authentication errors or invalid credentials while continuing to search healthy sources?",
    "What is the status of the networking issue?",
    "Is there evidence that Sunbeam supports quantum-network routing for production deployments?",
    "Summarise the discussion, latest decisions, and current status for traefik ingress cert refresh (snap-openstack#879) in under 250 words.",
    "Are there issues where Jira is marked Done, but the GitHub PR is Open or Launchpad is Fix Committed? Detail the contradictions.",
    "What is known about private or missing tracker objects referenced by open issues, and are their contents verifiable?",
    "When multiple issues or trackers reference the same pull request, how is that evidence reconciled and deduplicated?",
];

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let target = args.get(1).map(|s| s.as_str()).unwrap_or("1");

    let paths = Paths::resolve(None, None)?;
    let mut config = Config::load_or_default(&paths.config_file)?;

    // Support --concepts on|off and ISSUEMESH_CONCEPTS env var
    if let Ok(val) = std::env::var("ISSUEMESH_CONCEPTS") {
        config.concepts.enabled =
            val == "1" || val.to_lowercase() == "true" || val.to_lowercase() == "on";
    }
    for arg in &args {
        if arg == "--concepts=on" || arg == "--concepts-on" {
            config.concepts.enabled = true;
        } else if arg == "--concepts=off" || arg == "--concepts-off" {
            config.concepts.enabled = false;
        }
    }
    for i in 0..args.len() {
        if args[i] == "--concepts" && i + 1 < args.len() {
            if args[i + 1] == "on" || args[i + 1] == "true" || args[i + 1] == "1" {
                config.concepts.enabled = true;
            } else if args[i + 1] == "off" || args[i + 1] == "false" || args[i + 1] == "0" {
                config.concepts.enabled = false;
            }
        }
    }
    println!(
        "Native Concept Layer: {}",
        if config.concepts.enabled {
            "ENABLED"
        } else {
            "DISABLED"
        }
    );

    let secrets = Secrets::load_from_file(&paths.secrets_file)?;
    let store = Store::open(&paths.db_file)?;

    let app_state = Arc::new(AppState::new(paths.clone(), config, secrets, store));

    std::fs::create_dir_all("/tmp/benchmark_results")?;

    let indices: Vec<usize> = if target == "all" {
        (1..=30).collect()
    } else if target.contains("..") {
        let clean = target.replace("..=", "..");
        let parts: Vec<&str> = clean.split("..").collect();
        let start: usize = parts[0].parse().unwrap_or(1);
        let end: usize = parts[1].parse().unwrap_or(30);
        (start..=end).collect()
    } else if let Ok(idx) = target.parse::<usize>() {
        if (1..=30).contains(&idx) {
            vec![idx]
        } else {
            eprintln!("Invalid test index: {}", idx);
            return Ok(());
        }
    } else {
        eprintln!("Usage: cargo run --example run_benchmark -- [1..30 | all]");
        return Ok(());
    };

    for idx in indices {
        let question = QUESTIONS[idx - 1];
        println!("========================================================");
        println!("Running Test {:02}: {}", idx, question);
        println!("========================================================");

        let res = SearchOrchestrator::execute(Arc::clone(&app_state), None, question).await;

        let word_count = res
            .answer
            .as_deref()
            .map(|a| a.split_whitespace().count())
            .unwrap_or(0);

        println!("\nPlan Query: {}", res.plan.query);
        println!("Candidates Ranked: {}", res.ranked_results.len());
        println!("Relations Followed: {}", res.relations_followed.len());
        println!("Findings: {}", res.findings.len());
        for (i, r) in res.ranked_results.iter().take(10).enumerate() {
            println!(
                "  [{}] {} ({}) - {} [score: {:.2}]",
                i + 1,
                r.evidence.r#ref.to_key(),
                r.evidence.state,
                r.evidence.title,
                r.score
            );
        }

        println!("\n--- Synthesized Answer ({} words) ---", word_count);
        if let Some(ref a) = res.answer {
            println!("{}", a);
        } else {
            println!("<NO ANSWER RETURNED>");
        }
        println!("------------------------------------\n");

        let out_json = serde_json::to_string_pretty(&res)?;
        std::fs::write(
            format!("/tmp/benchmark_results/test_{:02}.json", idx),
            out_json,
        )?;
    }

    Ok(())
}
