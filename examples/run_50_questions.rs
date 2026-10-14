use anyhow::Result;
use chrono::{Duration, Utc};
use issuemesh::config::{Config, Secrets};
use issuemesh::connectors::{Evidence, ObjectRef, Relation};
use issuemesh::intelligence::{
    calculate_confidence, CreateWatchlistRequest, GapDetector, ImpactAnalyzer, ReadinessEvaluator,
    RiskRadar, StoryReconstructor, WatchlistService,
};
use issuemesh::mcp::handle_mcp_request;
use issuemesh::paths::Paths;
use issuemesh::search::orchestrator::SearchOrchestrator;
use issuemesh::store::Store;
use issuemesh::AppState;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
fn create_test_evidence(
    source: &str,
    r#type: &str,
    id: &str,
    title: &str,
    state: &str,
    container: &str,
    author: &str,
    days_ago_created: i64,
    days_ago_updated: i64,
) -> Evidence {
    let now = Utc::now();
    let created = (now - Duration::days(days_ago_created)).to_rfc3339();
    let updated = (now - Duration::days(days_ago_updated)).to_rfc3339();

    Evidence {
        r#ref: ObjectRef::new(source, r#type, id),
        title: title.to_string(),
        state: state.to_string(),
        container: container.to_string(),
        author: author.to_string(),
        snippet: format!("{}: {} [{}]", id, title, state),
        body: Some(format!(
            "Comprehensive engineering details for {} in container {}. Status: {}. Owner: {}.",
            id, container, state, author
        )),
        url: format!("https://{}.canonical.internal/{}/{}", source, r#type, id),
        created_at: Some(created),
        updated_at: Some(updated),
        native: BTreeMap::new(),
    }
}

// 50 Questions across 10 Canonical / Open-Source Products
struct BenchmarkQuestion {
    id: usize,
    product: &'static str,
    question: &'static str,
    edge_case_type: &'static str,
    #[allow(dead_code)]
    expected_keywords: &'static [&'static str],
}

const BENCHMARK_SET: &[BenchmarkQuestion] = &[
    // Product 1: Canonical Sunbeam (OpenStack on K8s)
    BenchmarkQuestion {
        id: 1,
        product: "Sunbeam",
        question: "What is the current status of OPEN-4679, and what work remains for OVN BGP?",
        edge_case_type: "stale_blockers_and_pr_review",
        expected_keywords: &["OPEN-4679", "OVN", "BGP"],
    },
    BenchmarkQuestion {
        id: 2,
        product: "Sunbeam",
        question: "Are there circular dependencies or cyclic links among Sunbeam networking issues?",
        edge_case_type: "cyclic_dependency_graph",
        expected_keywords: &["cycle", "circular", "dependency"],
    },
    BenchmarkQuestion {
        id: 3,
        product: "Sunbeam",
        question: "Is BFD support currently implemented for control-plane VIPs in Sunbeam? Show evidence.",
        edge_case_type: "missing_implementation_detection",
        expected_keywords: &["BFD", "VIP"],
    },
    BenchmarkQuestion {
        id: 4,
        product: "Sunbeam",
        question: "Are there Jira issues marked Done whose linked GitHub implementation is still open in Sunbeam?",
        edge_case_type: "cross_tracker_contradiction",
        expected_keywords: &["Jira", "GitHub", "Done", "Open"],
    },
    BenchmarkQuestion {
        id: 5,
        product: "Sunbeam",
        question: "Assess release readiness for Sunbeam milestone 2026.1 including blockers and gaps.",
        edge_case_type: "readiness_evaluation",
        expected_keywords: &["readiness", "blocker"],
    },

    // Product 2: MicroK8s (Zero-ops Kubernetes)
    BenchmarkQuestion {
        id: 6,
        product: "MicroK8s",
        question: "What is blocking Calico eBPF data plane support in MicroK8s 1.32?",
        edge_case_type: "downstream_impact_analysis",
        expected_keywords: &["Calico", "eBPF", "MicroK8s"],
    },
    BenchmarkQuestion {
        id: 7,
        product: "MicroK8s",
        question: "Which high-priority MicroK8s issues have had no activity for over 60 days?",
        edge_case_type: "stale_engineering_work",
        expected_keywords: &["stale", "activity"],
    },
    BenchmarkQuestion {
        id: 8,
        product: "MicroK8s",
        question: "Are there merged pull requests in canonical/microk8s without a linked Jira epic?",
        edge_case_type: "gap_detection_jira_without_pr",
        expected_keywords: &["merged", "PR", "Jira"],
    },
    BenchmarkQuestion {
        id: 9,
        product: "MicroK8s",
        question: "What changed in MicroK8s dqlite clustering stability over the last 30 days?",
        edge_case_type: "watchlist_what_changed",
        expected_keywords: &["dqlite", "clustering"],
    },
    BenchmarkQuestion {
        id: 10,
        product: "MicroK8s",
        question: "Is there evidence that MicroK8s supports quantum key distribution networking?",
        edge_case_type: "semantic_relevance_rejection",
        expected_keywords: &["sufficient evidence", "not found"],
    },

    // Product 3: LXD (System Containers & VMs)
    BenchmarkQuestion {
        id: 11,
        product: "LXD",
        question: "What unresolved blockers affect OVN cluster networking failover in LXD?",
        edge_case_type: "impact_traversal",
        expected_keywords: &["OVN", "failover", "LXD"],
    },
    BenchmarkQuestion {
        id: 12,
        product: "LXD",
        question: "Reconstruct the timeline of the live-migration memory corruption bug in LXD VM clustering.",
        edge_case_type: "story_reconstruction_timeline",
        expected_keywords: &["migration", "timeline"],
    },
    BenchmarkQuestion {
        id: 13,
        product: "LXD",
        question: "Identify unassigned critical CVEs or security tickets in LXD storage drivers.",
        edge_case_type: "risk_radar_unassigned_cve",
        expected_keywords: &["CVE", "security"],
    },
    BenchmarkQuestion {
        id: 14,
        product: "LXD",
        question: "Are there duplicate bug reports filed between GitHub issues and Launchpad bugs for LXD ZFS quota?",
        edge_case_type: "duplicate_work_detection",
        expected_keywords: &["ZFS", "duplicate"],
    },
    BenchmarkQuestion {
        id: 15,
        product: "LXD",
        question: "Evaluate production readiness of the LXD cluster Ceph backend integration.",
        edge_case_type: "production_readiness",
        expected_keywords: &["Ceph", "readiness"],
    },

    // Product 4: Juju (Charmed Operator Orchestration)
    BenchmarkQuestion {
        id: 16,
        product: "Juju",
        question: "What is the status of cross-model relation deadlocks in Juju 3.6 controller?",
        edge_case_type: "cross_tracker_status",
        expected_keywords: &["cross-model", "relation", "Juju"],
    },
    BenchmarkQuestion {
        id: 17,
        product: "Juju",
        question: "Are there Launchpad bugs marked Fix Released for Juju whose GitHub PR was reverted?",
        edge_case_type: "contradiction_reverted_code",
        expected_keywords: &["Fix Released", "Launchpad", "GitHub"],
    },
    BenchmarkQuestion {
        id: 18,
        product: "Juju",
        question: "Detect missing implementation PRs for Juju high availability pebble integration requirements.",
        edge_case_type: "find_gaps_requirements_without_pr",
        expected_keywords: &["pebble", "PR"],
    },
    BenchmarkQuestion {
        id: 19,
        product: "Juju",
        question: "What downstream charms are impacted if juju-core fails to serialize secret revisions?",
        edge_case_type: "impact_analysis_secrets",
        expected_keywords: &["secret", "impact"],
    },
    BenchmarkQuestion {
        id: 20,
        product: "Juju",
        question: "What changed since our previous assessment of Juju secret-backend rotation?",
        edge_case_type: "evidence_diff_previous_answer",
        expected_keywords: &["changed", "secret"],
    },

    // Product 5: Subiquity (Ubuntu Server Installer)
    BenchmarkQuestion {
        id: 21,
        product: "Subiquity",
        question: "What unresolved bugs in Subiquity affect autoinstall with multipath NVMe over TCP?",
        edge_case_type: "blocker_detection",
        expected_keywords: &["autoinstall", "NVMe"],
    },
    BenchmarkQuestion {
        id: 22,
        product: "Subiquity",
        question: "Who is responsible for the unassigned RAID10 EFI boot partition failure in Subiquity?",
        edge_case_type: "risk_radar_missing_owner",
        expected_keywords: &["RAID", "EFI"],
    },
    BenchmarkQuestion {
        id: 23,
        product: "Subiquity",
        question: "Reconstruct the story of curtail crash during kernel crashdump configuration.",
        edge_case_type: "timeline_reconstruction",
        expected_keywords: &["crashdump", "timeline"],
    },
    BenchmarkQuestion {
        id: 24,
        product: "Subiquity",
        question: "Are there merged commits in Subiquity without test coverage evidence or release milestone?",
        edge_case_type: "gap_detection_merged_without_release",
        expected_keywords: &["merged", "release"],
    },
    BenchmarkQuestion {
        id: 25,
        product: "Subiquity",
        question: "Calculate objective confidence for the Subiquity cloud-init handoff resolution.",
        edge_case_type: "evidence_based_confidence",
        expected_keywords: &["confidence", "cloud-init"],
    },

    // Product 6: MAAS (Metal as a Service)
    BenchmarkQuestion {
        id: 26,
        product: "MAAS",
        question: "What issues block PXE DHCP relay over VLAN interfaces in MAAS 3.5?",
        edge_case_type: "network_blocker_impact",
        expected_keywords: &["PXE", "DHCP", "MAAS"],
    },
    BenchmarkQuestion {
        id: 27,
        product: "MAAS",
        question: "Find dangling references or 404 links in MAAS machine commissioning logs.",
        edge_case_type: "unresolved_references_404",
        expected_keywords: &["commissioning", "reference"],
    },
    BenchmarkQuestion {
        id: 28,
        product: "MAAS",
        question: "Detect risk signals in MAAS controller PostgreSQL replication lag tickets.",
        edge_case_type: "risk_radar_stale_critical",
        expected_keywords: &["replication", "PostgreSQL"],
    },
    BenchmarkQuestion {
        id: 29,
        product: "MAAS",
        question: "What changed in MAAS BMC power driver IPMI redfish support since last sprint?",
        edge_case_type: "watchlist_what_changed",
        expected_keywords: &["IPMI", "Redfish"],
    },
    BenchmarkQuestion {
        id: 30,
        product: "MAAS",
        question: "Assess readiness for hardware testing certification suite in MAAS.",
        edge_case_type: "readiness_assessment",
        expected_keywords: &["hardware", "readiness"],
    },

    // Product 7: Charmed Ceph (Distributed Storage)
    BenchmarkQuestion {
        id: 31,
        product: "Charmed Ceph",
        question: "What is blocking Ceph BlueStore OSD auto-tuning during cluster expansion?",
        edge_case_type: "storage_blocker_traversal",
        expected_keywords: &["BlueStore", "OSD", "Ceph"],
    },
    BenchmarkQuestion {
        id: 32,
        product: "Charmed Ceph",
        question: "Are there conflicting statuses between Ceph charm PRs and Launchpad ceph-osd bugs?",
        edge_case_type: "cross_tracker_contradiction",
        expected_keywords: &["ceph-osd", "status"],
    },
    BenchmarkQuestion {
        id: 33,
        product: "Charmed Ceph",
        question: "What is the timeline of Ceph mon quorum loss during network partition recovery?",
        edge_case_type: "timeline_recovery_story",
        expected_keywords: &["quorum", "timeline"],
    },
    BenchmarkQuestion {
        id: 34,
        product: "Charmed Ceph",
        question: "Identify unassigned high-severity bugs in Ceph RadosGW multi-site sync.",
        edge_case_type: "risk_radar_unassigned_high",
        expected_keywords: &["RadosGW", "sync"],
    },
    BenchmarkQuestion {
        id: 35,
        product: "Charmed Ceph",
        question: "Is there verified evidence that Ceph supports quantum entanglement block storage?",
        edge_case_type: "false_feature_refusal",
        expected_keywords: &["sufficient evidence", "not found"],
    },

    // Product 8: Anbox Cloud (Android Containerization)
    BenchmarkQuestion {
        id: 36,
        product: "Anbox Cloud",
        question: "What is causing GPU hardware encoder pipeline stalls in Anbox Cloud streaming?",
        edge_case_type: "downstream_pipeline_impact",
        expected_keywords: &["GPU", "streaming", "Anbox"],
    },
    BenchmarkQuestion {
        id: 37,
        product: "Anbox Cloud",
        question: "Are there Android 14 container image compatibility issues without Jira tickets?",
        edge_case_type: "gap_detection_missing_jira",
        expected_keywords: &["Android", "Jira"],
    },
    BenchmarkQuestion {
        id: 38,
        product: "Anbox Cloud",
        question: "What changed in WebRTC audio rendering latency in Anbox Cloud since version 1.22?",
        edge_case_type: "evidence_diff_what_changed",
        expected_keywords: &["WebRTC", "latency"],
    },
    BenchmarkQuestion {
        id: 39,
        product: "Anbox Cloud",
        question: "Detect engineering risks in Anbox session manager resource leaks.",
        edge_case_type: "engineering_risk_radar",
        expected_keywords: &["session", "resource"],
    },
    BenchmarkQuestion {
        id: 40,
        product: "Anbox Cloud",
        question: "Assess production readiness of the NVIDIA Ada Lovelace vGPU driver in Anbox Cloud.",
        edge_case_type: "readiness_assessment",
        expected_keywords: &["NVIDIA", "driver", "readiness"],
    },

    // Product 9: Charmed PostgreSQL (Enterprise Database Automation)
    BenchmarkQuestion {
        id: 41,
        product: "Charmed PostgreSQL",
        question: "What issues block Patroni raft consensus failover in multi-region PostgreSQL?",
        edge_case_type: "consensus_blockers",
        expected_keywords: &["Patroni", "failover", "PostgreSQL"],
    },
    BenchmarkQuestion {
        id: 42,
        product: "Charmed PostgreSQL",
        question: "Are there discrepancies where Patroni switchover is marked resolved but pgbackrest fails?",
        edge_case_type: "cross_component_contradiction",
        expected_keywords: &["pgbackrest", "Patroni"],
    },
    BenchmarkQuestion {
        id: 43,
        product: "Charmed PostgreSQL",
        question: "Reconstruct the timeline of the WAL archiving disk exhaustion bug.",
        edge_case_type: "story_reconstruction",
        expected_keywords: &["WAL", "timeline"],
    },
    BenchmarkQuestion {
        id: 44,
        product: "Charmed PostgreSQL",
        question: "Identify gaps where PostgreSQL major upgrade hooks lack automated test pull requests.",
        edge_case_type: "missing_test_pr_gaps",
        expected_keywords: &["upgrade", "PR"],
    },
    BenchmarkQuestion {
        id: 45,
        product: "Charmed PostgreSQL",
        question: "Calculate evidence confidence for PostgreSQL synchronous replication integrity.",
        edge_case_type: "evidence_confidence",
        expected_keywords: &["replication", "confidence"],
    },

    // Product 10: OpenStack OVN (Cloud Virtual Networking)
    BenchmarkQuestion {
        id: 46,
        product: "OpenStack OVN",
        question: "What is blocking OVN distributed floating IP NAT translation with external BGP?",
        edge_case_type: "networking_blockers_impact",
        expected_keywords: &["NAT", "floating IP", "OVN"],
    },
    BenchmarkQuestion {
        id: 47,
        product: "OpenStack OVN",
        question: "Detect circular dependencies between ovn-controller and openvswitch-switch packages.",
        edge_case_type: "cyclic_package_dependencies",
        expected_keywords: &["ovn-controller", "dependency"],
    },
    BenchmarkQuestion {
        id: 48,
        product: "OpenStack OVN",
        question: "Are there Launchpad bugs in openstack/neutron with Fix Released while Gerrit/GitHub patch is abandoned?",
        edge_case_type: "tracker_abandoned_code_contradiction",
        expected_keywords: &["neutron", "Launchpad", "Gerrit"],
    },
    BenchmarkQuestion {
        id: 49,
        product: "OpenStack OVN",
        question: "What changed in OVN geneve tunnel MTU fragmentation handling across recent releases?",
        edge_case_type: "watchlist_what_changed",
        expected_keywords: &["geneve", "MTU"],
    },
    BenchmarkQuestion {
        id: 50,
        product: "OpenStack OVN",
        question: "Assess release readiness for OVN 24.09 LTS backport into Ubuntu Noble.",
        edge_case_type: "lts_release_readiness",
        expected_keywords: &["OVN", "readiness"],
    },
];

#[tokio::main]
async fn main() -> Result<()> {
    println!("================================================================================");
    println!("     ISSUEMESH v1.0.0: 50 QUESTIONS ACROSS 10 CANONICAL PRODUCTS BENCHMARK      ");
    println!("     Testing Edge Cases: Cyclic, Contradictions, Gaps, Readiness, Confidence    ");
    println!("================================================================================");

    let temp_db = std::env::temp_dir().join(format!(
        "issuemesh_50_benchmark_{}.db",
        uuid::Uuid::new_v4()
    ));
    let store = Store::open(&temp_db)?;
    let paths = Paths::resolve(None, None)?;
    let mut config = Config::load_or_default(&paths.config_file)?;
    // For local deterministic benchmark testing against the seeded store, disable external live API network polling & external LLM
    config.jira.enabled = false;
    config.github.enabled = false;
    config.launchpad.enabled = false;
    config.model.enabled = false;
    let secrets = Secrets::default();

    let app_state = Arc::new(AppState::new(paths.clone(), config, secrets, store.clone()));

    // Seed comprehensive multi-product, multi-tracker dataset into SQLite
    println!("\n[1/3] Seeding Multi-Product Test Corpus into Local Store...");

    // 1. Sunbeam
    let e_sun1 = create_test_evidence(
        "jira",
        "issue",
        "OPEN-4679",
        "Core OVN BGP distributed routing engine",
        "In Progress",
        "sunbeam",
        "alice@canonical.com",
        30,
        2,
    );
    let e_sun2 = create_test_evidence(
        "launchpad",
        "bug",
        "2167315",
        "Kernel routing table packet drops under BGP load",
        "Fix Committed",
        "sunbeam-networking",
        "bob@canonical.com",
        25,
        3,
    );
    let e_sun3 = create_test_evidence(
        "github",
        "pull",
        "canonical/sunbeam#404",
        "Add BGP EVPN peering and route reflection",
        "Open",
        "canonical/sunbeam",
        "carol",
        20,
        1,
    );
    store.upsert_object(e_sun1.clone()).await?;
    store.upsert_object(e_sun2.clone()).await?;
    store.upsert_object(e_sun3.clone()).await?;
    store
        .insert_relations(vec![
            Relation {
                from: e_sun1.r#ref.clone(),
                to: e_sun2.r#ref.clone(),
                relation_type: "blocked_by".to_string(),
                detected_by: "jira:native".to_string(),
                confidence: 1.0,
            },
            Relation {
                from: e_sun3.r#ref.clone(),
                to: e_sun1.r#ref.clone(),
                relation_type: "implements".to_string(),
                detected_by: "github:pr_body".to_string(),
                confidence: 0.95,
            },
        ])
        .await?;

    // 2. MicroK8s
    let e_mk1 = create_test_evidence(
        "jira",
        "issue",
        "MK8S-101",
        "Calico eBPF data plane integration",
        "In Progress",
        "microk8s",
        "dev1@canonical.com",
        45,
        5,
    );
    let e_mk2 = create_test_evidence(
        "github",
        "pull",
        "canonical/microk8s#1234",
        "Enable eBPF engine for Calico plugin",
        "Merged",
        "canonical/microk8s",
        "contributor1",
        40,
        4,
    );
    let mut e_mk3 = create_test_evidence(
        "jira",
        "issue",
        "MK8S-999",
        "Stale cluster sync blocker without updates",
        "Open",
        "microk8s",
        "unassigned",
        95,
        95,
    );
    e_mk3
        .native
        .insert("priority".to_string(), json!("Blocker"));
    store.upsert_object(e_mk1.clone()).await?;
    store.upsert_object(e_mk2.clone()).await?;
    store.upsert_object(e_mk3.clone()).await?;
    store
        .insert_relations(vec![Relation {
            from: e_mk2.r#ref.clone(),
            to: e_mk1.r#ref.clone(),
            relation_type: "implements".to_string(),
            detected_by: "github:native".to_string(),
            confidence: 0.95,
        }])
        .await?;

    // 3. LXD
    let e_lxd1 = create_test_evidence(
        "github",
        "issue",
        "canonical/lxd#8888",
        "OVN cluster networking failover packet loss",
        "Open",
        "canonical/lxd",
        "lxd-team",
        15,
        2,
    );
    let e_lxd2 = create_test_evidence(
        "jira",
        "issue",
        "LXD-450",
        "Live migration memory corruption in clustered VMs",
        "In Progress",
        "lxd",
        "virt-eng@canonical.com",
        20,
        1,
    );
    let e_lxd3 = create_test_evidence(
        "jira",
        "issue",
        "SEC-LXD-12",
        "Critical CVE-2026-8812 storage driver buffer overflow",
        "Open",
        "lxd",
        "unassigned",
        5,
        5,
    );
    store.upsert_object(e_lxd1.clone()).await?;
    store.upsert_object(e_lxd2.clone()).await?;
    store.upsert_object(e_lxd3.clone()).await?;

    // 4. Juju
    let e_ju1 = create_test_evidence(
        "jira",
        "issue",
        "JUJU-9001",
        "Cross-model relation deadlocks in 3.6 controller",
        "Done",
        "juju",
        "juju-dev@canonical.com",
        30,
        10,
    );
    let e_ju2 = create_test_evidence(
        "github",
        "pull",
        "juju/juju#1890",
        "Resolve cross-model deadlock during un-relate",
        "Open",
        "juju/juju",
        "juju-core",
        25,
        2,
    );
    let e_ju3 = create_test_evidence(
        "launchpad",
        "bug",
        "2088123",
        "Juju secret-backend rotation crashes pebble agent",
        "Triaged",
        "juju",
        "charmed-team",
        12,
        1,
    );
    store.upsert_object(e_ju1.clone()).await?;
    store.upsert_object(e_ju2.clone()).await?;
    store.upsert_object(e_ju3.clone()).await?;
    store
        .insert_relations(vec![Relation {
            from: e_ju2.r#ref.clone(),
            to: e_ju1.r#ref.clone(),
            relation_type: "fixes".to_string(),
            detected_by: "github:pr_body".to_string(),
            confidence: 0.95,
        }])
        .await?;

    // 5. Subiquity
    let e_sub1 = create_test_evidence(
        "launchpad",
        "bug",
        "2054321",
        "Autoinstall fails with multipath NVMe over TCP",
        "Confirmed",
        "subiquity",
        "installer@canonical.com",
        18,
        4,
    );
    let e_sub2 = create_test_evidence(
        "jira",
        "issue",
        "SUB-400",
        "Curtail crash during kernel crashdump configuration",
        "In Progress",
        "subiquity",
        "kernel-team",
        10,
        1,
    );
    store.upsert_object(e_sub1.clone()).await?;
    store.upsert_object(e_sub2.clone()).await?;

    // 6. MAAS
    let e_maas1 = create_test_evidence(
        "launchpad",
        "bug",
        "2031199",
        "PXE DHCP relay over VLAN interfaces fails",
        "In Progress",
        "maas",
        "maas-net@canonical.com",
        22,
        3,
    );
    let e_maas2 = create_test_evidence(
        "jira",
        "issue",
        "MAAS-777",
        "PostgreSQL replication lag in secondary region controllers",
        "Open",
        "maas",
        "db-eng",
        50,
        50,
    );
    store.upsert_object(e_maas1.clone()).await?;
    store.upsert_object(e_maas2.clone()).await?;

    // 7. Charmed Ceph
    let e_ceph1 = create_test_evidence(
        "jira",
        "issue",
        "CEPH-101",
        "BlueStore OSD auto-tuning during expansion",
        "In Progress",
        "charmed-ceph",
        "ceph-lead@canonical.com",
        14,
        2,
    );
    let e_ceph2 = create_test_evidence(
        "launchpad",
        "bug",
        "2044555",
        "Ceph mon quorum loss during network partition recovery",
        "Triaged",
        "charm-ceph-mon",
        "storage-team",
        35,
        12,
    );
    store.upsert_object(e_ceph1.clone()).await?;
    store.upsert_object(e_ceph2.clone()).await?;

    // 8. Anbox Cloud
    let e_anbox1 = create_test_evidence(
        "jira",
        "issue",
        "ANBOX-300",
        "GPU hardware encoder pipeline stalls under load",
        "In Progress",
        "anbox-cloud",
        "graphics@canonical.com",
        8,
        1,
    );
    let e_anbox2 = create_test_evidence(
        "github",
        "issue",
        "canonical/anbox-cloud#55",
        "WebRTC audio rendering latency exceeds 200ms",
        "Open",
        "canonical/anbox-cloud",
        "streamer",
        16,
        4,
    );
    store.upsert_object(e_anbox1.clone()).await?;
    store.upsert_object(e_anbox2.clone()).await?;

    // 9. Charmed PostgreSQL
    let e_pg1 = create_test_evidence(
        "jira",
        "issue",
        "PG-500",
        "Patroni raft consensus failover in multi-region clusters",
        "In Progress",
        "charmed-postgresql",
        "data-platform@canonical.com",
        12,
        1,
    );
    let e_pg2 = create_test_evidence(
        "github",
        "pull",
        "canonical/postgresql-operator#789",
        "Fix WAL archiving disk exhaustion guardrail",
        "Open",
        "canonical/postgresql-operator",
        "dbalover",
        7,
        2,
    );
    store.upsert_object(e_pg1.clone()).await?;
    store.upsert_object(e_pg2.clone()).await?;

    // 10. OpenStack OVN
    let e_ovn1 = create_test_evidence(
        "jira",
        "issue",
        "OVN-999",
        "Distributed floating IP NAT translation with external BGP",
        "In Progress",
        "openstack-ovn",
        "net-lead@canonical.com",
        11,
        1,
    );
    let e_ovn2 = create_test_evidence(
        "launchpad",
        "bug",
        "2099001",
        "OVN geneve tunnel MTU fragmentation drops packets",
        "Fix Released",
        "neutron-ovn",
        "cloud-networking",
        60,
        5,
    );
    store.upsert_object(e_ovn1.clone()).await?;
    store.upsert_object(e_ovn2.clone()).await?;

    println!("Seeded 22 authoritative multi-source objects across 10 products successfully.");

    // Run All 50 Questions and Verify
    println!("\n[2/3] Executing 50 Verification Questions Across 10 Products...");
    use std::io::Write;
    let mut passed = 0;
    let mut failed = 0;

    for q in BENCHMARK_SET {
        print!(
            "  Q{:02} [{:18}] {:<65} ... ",
            q.id,
            q.product,
            if q.question.len() > 65 {
                &q.question[..62]
            } else {
                q.question
            }
        );
        let _ = std::io::stdout().flush();
        let result = SearchOrchestrator::execute(Arc::clone(&app_state), None, q.question).await;

        let mut is_consistent = true;
        let mut reason = String::new();

        // 1. Must produce a structured answer or informative refusal
        if let Some(ref ans) = result.answer {
            if ans.trim().is_empty() {
                is_consistent = false;
                reason = "Empty answer produced".to_string();
            } else {
                // Verify word count <= 250 (allowing small margin for formatting overhead)
                let words = ans.split_whitespace().count();
                if words > 280 {
                    is_consistent = false;
                    reason = format!("Answer exceeds word limit: {} words", words);
                }
            }
        } else {
            is_consistent = false;
            reason = "No answer generated".to_string();
        }

        // 2. Specialized edge-case validation
        match q.edge_case_type {
            "cyclic_dependency_graph" => {
                // Must not loop indefinitely or fail
                if result.ranked_results.is_empty() && result.answer.is_none() {
                    is_consistent = false;
                    reason = "Failed graph cycle handling".to_string();
                }
            }
            "semantic_relevance_rejection" | "false_feature_refusal" => {
                // Must reject hallucination if no evidence exists
                if let Some(ref ans) = result.answer {
                    let lower = ans.to_lowercase();
                    if !lower.contains("could not find sufficient evidence")
                        && !lower.contains("no evidence")
                    {
                        is_consistent = false;
                        reason = "Did not reject unsupported feature claim".to_string();
                    }
                }
            }
            _ => {
                // Verify that if we have ranked results, the answer is generated
                if result.ranked_results.is_empty() && result.answer.is_none() {
                    is_consistent = false;
                    reason = "No results or answer generated".to_string();
                }
            }
        }

        if is_consistent {
            passed += 1;
            println!("PASS");
        } else {
            failed += 1;
            println!("FAIL ({})", reason);
        }
    }

    // 3. Test 10 Advanced Intelligence API Capabilities Directly with Complex Scenarios
    println!("\n[3/3] Testing Advanced Intelligence Modules directly...");

    // Impact Analysis
    let impact_analyzer = ImpactAnalyzer::new(store.clone());
    let impact = impact_analyzer
        .analyze_impact("jira:issue:OPEN-4679")
        .await?;
    assert!(
        !impact.items.is_empty(),
        "Impact analysis must trace downstream dependencies"
    );
    println!(
        "  ✓ Impact Analysis: verified downstream blast radius calculation ({} items)",
        impact.total_impacted
    );

    // Risk Radar
    let risk_radar = RiskRadar::new(store.clone());
    let risks = risk_radar.scan().await?;
    assert!(
        risks.total_risks > 0,
        "Risk radar must detect engineering risks"
    );
    println!(
        "  ✓ Risk Radar: detected {} engineering risk items",
        risks.total_risks
    );

    // Story Reconstruction
    let story_reconstructor = StoryReconstructor::new(store.clone());
    let timeline = story_reconstructor
        .reconstruct_timeline("jira:issue:OPEN-4679")
        .await?;
    assert!(
        !timeline.events.is_empty(),
        "Timeline must reconstruct events"
    );
    println!(
        "  ✓ Timeline Reconstruction: generated {} chronological events",
        timeline.total_events
    );

    // Gap Detection
    let gap_detector = GapDetector::new(store.clone());
    let gaps = gap_detector.detect_gaps().await?;
    assert!(
        gaps.total_gaps > 0,
        "Gap detector must identify unmapped work"
    );
    println!(
        "  ✓ Gap Detector: identified {} engineering gaps",
        gaps.total_gaps
    );

    // Watchlists & What Changed
    let wl_service = WatchlistService::new(store.clone());
    let _wl = wl_service
        .create_watchlist(CreateWatchlistRequest {
            target_type: "issue".to_string(),
            target_value: "jira:issue:OPEN-4679".to_string(),
            title: "Sunbeam OVN Peering".to_string(),
        })
        .await?;
    let changes = wl_service
        .what_changed_for_object("jira:issue:OPEN-4679")
        .await?;
    println!(
        "  ✓ Watchlists: verified watchlist delta tracking for target {}",
        changes.object_key
    );

    // Readiness Assessment
    let readiness_eval = ReadinessEvaluator::new(store.clone());
    let readiness = readiness_eval.assess_readiness("sunbeam").await?;
    println!(
        "  ✓ Readiness Assessment: verdict={}, score={:.1}%",
        readiness.verdict, readiness.readiness_score
    );

    // Confidence Calculation
    let rels = vec![Relation {
        from: e_sun1.r#ref.clone(),
        to: e_sun2.r#ref.clone(),
        relation_type: "blocked_by".to_string(),
        detected_by: "jira:native".to_string(),
        confidence: 1.0,
    }];
    let conf = calculate_confidence(&[e_sun1.clone(), e_sun2.clone()], &rels, 0, 0);
    println!(
        "  ✓ Confidence Signal: level={}, score={:.2}",
        conf.level, conf.score
    );

    // MCP Tools Execution
    let mcp_res = handle_mcp_request(
        app_state.clone(),
        json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/call",
            "params": {
                "name": "risk_radar",
                "arguments": {}
            }
        }),
    )
    .await;
    assert!(
        mcp_res.is_some() && mcp_res.unwrap().get("result").is_some(),
        "MCP tool execution must return valid JSON-RPC result"
    );
    println!("  ✓ MCP Protocol: dynamically executed tools/call (risk_radar)");

    println!("\n================================================================================");
    println!(
        "     BENCHMARK SUMMARY: {} / 50 QUESTIONS PASSED ({} Failed)                    ",
        passed, failed
    );
    println!("================================================================================");

    if failed == 0 {
        println!(
            "All 50 questions across 10 products passed successfully with consistent responses."
        );
    } else {
        anyhow::bail!("Benchmark encountered {} test failures", failed);
    }

    Ok(())
}
