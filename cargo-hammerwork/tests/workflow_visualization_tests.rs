use cargo_hammerwork::commands::workflow::{JobNode, WorkflowCommand};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Create test job nodes for visualization testing
fn create_test_jobs() -> Vec<JobNode> {
    vec![
        JobNode {
            id: "12345678-1234-1234-1234-123456789012".to_string(),
            queue_name: "data-processing".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec!["87654321-4321-4321-4321-210987654321".to_string()],
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("data-pipeline".to_string()),
        },
        JobNode {
            id: "87654321-4321-4321-4321-210987654321".to_string(),
            queue_name: "data-transform".to_string(),
            status: "Running".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["12345678-1234-1234-1234-123456789012".to_string()],
            dependents: vec![
                "abcdef12-abcd-abcd-abcd-abcdef123456".to_string(),
                "fedcba98-fedc-fedc-fedc-fedcba987654".to_string(),
            ],
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("data-pipeline".to_string()),
        },
        JobNode {
            id: "abcdef12-abcd-abcd-abcd-abcdef123456".to_string(),
            queue_name: "data-export".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["87654321-4321-4321-4321-210987654321".to_string()],
            dependents: vec![],
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("data-pipeline".to_string()),
        },
        JobNode {
            id: "fedcba98-fedc-fedc-fedc-fedcba987654".to_string(),
            queue_name: "cleanup".to_string(),
            status: "Failed".to_string(),
            dependency_status: "failed".to_string(),
            depends_on: vec!["87654321-4321-4321-4321-210987654321".to_string()],
            dependents: vec![],
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("data-pipeline".to_string()),
        },
    ]
}

/// Create a workflow command for testing
fn create_test_workflow_command() -> WorkflowCommand {
    WorkflowCommand::List {
        database_url: None,
        limit: None,
        running: false,
        completed: false,
        failed: false,
    }
}

#[test]
fn test_json_array_parsing() {
    let cmd = create_test_workflow_command();

    // Test various JSON array scenarios
    let test_cases = vec![
        (
            serde_json::json!(["job1", "job2", "job3"]),
            vec!["job1", "job2", "job3"],
        ),
        (serde_json::json!([]), vec![]),
        (serde_json::json!(["single"]), vec!["single"]),
        (
            serde_json::json!(["job1", 123, "job2", null, true, "job3"]),
            vec!["job1", "job2", "job3"],
        ),
    ];

    for (input, expected) in test_cases {
        let result = cmd.parse_json_array(Some(input)).unwrap();
        assert_eq!(result, expected);
    }

    // Test None input
    assert!(cmd.parse_json_array(None).unwrap().is_empty());

    // Test non-array JSON
    assert!(
        cmd.parse_json_array(Some(serde_json::json!({"not": "array"})))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn test_dependency_level_calculation_complex() {
    let cmd = create_test_workflow_command();
    let jobs = create_test_jobs();

    let levels = cmd.calculate_dependency_levels(&jobs);

    // Should have multiple levels
    assert!(!levels.is_empty());

    // Verify level structure makes sense
    let mut level_numbers: Vec<usize> = levels.iter().map(|(level, _)| *level).collect();
    level_numbers.sort();
    level_numbers.dedup();

    // Should start at level 0
    assert_eq!(level_numbers[0], 0);

    // Should have reasonable number of levels for our test data
    assert!(level_numbers.len() <= 4); // Maximum possible levels for our test data

    // Verify each level has jobs
    for (level, jobs_at_level) in &levels {
        assert!(
            !jobs_at_level.is_empty(),
            "Level {} should have jobs",
            level
        );
    }
}

#[test]
fn test_dependency_tree_structure() {
    let jobs = create_test_jobs();

    // Create a mapping for easier testing
    let job_map: HashMap<String, &JobNode> = jobs.iter().map(|job| (job.id.clone(), job)).collect();

    // Test parent-child relationships
    let root_job = job_map.get("12345678-1234-1234-1234-123456789012").unwrap();
    assert!(
        root_job.depends_on.is_empty(),
        "Root job should have no dependencies"
    );
    assert!(
        !root_job.dependents.is_empty(),
        "Root job should have dependents"
    );

    let middle_job = job_map.get("87654321-4321-4321-4321-210987654321").unwrap();
    assert!(
        !middle_job.depends_on.is_empty(),
        "Middle job should have dependencies"
    );
    assert!(
        !middle_job.dependents.is_empty(),
        "Middle job should have dependents"
    );

    let leaf_job = job_map.get("abcdef12-abcd-abcd-abcd-abcdef123456").unwrap();
    assert!(
        !leaf_job.depends_on.is_empty(),
        "Leaf job should have dependencies"
    );
    assert!(
        leaf_job.dependents.is_empty(),
        "Leaf job should have no dependents"
    );
}

#[test]
fn test_job_node_status_variations() {
    let statuses = vec![
        ("Completed", "none"),
        ("Running", "satisfied"),
        ("Pending", "waiting"),
        ("Failed", "failed"),
        ("TimedOut", "failed"),
        ("Retrying", "waiting"),
    ];

    for (job_status, dep_status) in statuses {
        let job = JobNode {
            id: format!("job-{}", job_status.to_lowercase()),
            queue_name: "test-queue".to_string(),
            status: job_status.to_string(),
            dependency_status: dep_status.to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("test-workflow".to_string()),
            workflow_name: Some("test".to_string()),
        };

        assert_eq!(job.status, job_status);
        assert_eq!(job.dependency_status, dep_status);
    }
}

#[test]
fn test_workflow_command_database_url_priority() {
    use cargo_hammerwork::config::Config;

    let config_with_url = Config {
        database_url: Some("postgres://config/db".to_string()),
        default_queue: None,
        default_limit: None,
        log_level: None,
        connection_pool_size: None,
    };

    let config_without_url = Config {
        database_url: None,
        default_queue: None,
        default_limit: None,
        log_level: None,
        connection_pool_size: None,
    };

    // Test command with URL overrides config
    let cmd_with_url = WorkflowCommand::Graph {
        database_url: Some("postgres://command/db".to_string()),
        workflow_id: "test".to_string(),
        format: None,
    };

    let result = cmd_with_url.get_database_url(&config_with_url).unwrap();
    assert_eq!(result, "postgres://command/db");

    // Test command without URL falls back to config
    let cmd_without_url = WorkflowCommand::Graph {
        database_url: None,
        workflow_id: "test".to_string(),
        format: None,
    };

    let result = cmd_without_url.get_database_url(&config_with_url).unwrap();
    assert_eq!(result, "postgres://config/db");

    // Test error when neither command nor config has URL
    let result = cmd_without_url.get_database_url(&config_without_url);
    assert!(result.is_err());
}

#[test]
fn test_graph_format_options() {
    let formats = vec!["text", "dot", "mermaid", "json"];

    for format in formats {
        let cmd = WorkflowCommand::Graph {
            database_url: None,
            workflow_id: "test-workflow".to_string(),
            format: Some(format.to_string()),
        };

        if let WorkflowCommand::Graph {
            format: cmd_format, ..
        } = cmd
        {
            assert_eq!(cmd_format, Some(format.to_string()));
        } else {
            panic!("Expected Graph command");
        }
    }
}

#[test]
fn test_all_workflow_command_variants_structure() {
    // Test that all command variants have the expected structure
    let commands = vec![
        (
            "List",
            WorkflowCommand::List {
                database_url: Some("postgres://test".to_string()),
                limit: Some(100),
                running: true,
                completed: false,
                failed: true,
            },
        ),
        (
            "Show",
            WorkflowCommand::Show {
                database_url: Some("postgres://test".to_string()),
                workflow_id: "workflow-123".to_string(),
                dependencies: true,
            },
        ),
        (
            "Create",
            WorkflowCommand::Create {
                database_url: Some("postgres://test".to_string()),
                name: "test-workflow".to_string(),
                failure_policy: Some("continue_on_failure".to_string()),
                metadata: Some(r#"{"env": "production"}"#.to_string()),
            },
        ),
        (
            "Cancel",
            WorkflowCommand::Cancel {
                database_url: Some("postgres://test".to_string()),
                workflow_id: "workflow-456".to_string(),
                force: true,
            },
        ),
        (
            "Dependencies",
            WorkflowCommand::Dependencies {
                database_url: Some("postgres://test".to_string()),
                job_id: "job-789".to_string(),
                tree: true,
                dependents: true,
            },
        ),
        (
            "Graph",
            WorkflowCommand::Graph {
                database_url: Some("postgres://test".to_string()),
                workflow_id: "workflow-abc".to_string(),
                format: Some("mermaid".to_string()),
            },
        ),
    ];

    for (name, command) in commands {
        // Test that each command can be matched correctly
        match command {
            WorkflowCommand::List { .. } => assert_eq!(name, "List"),
            WorkflowCommand::Show { .. } => assert_eq!(name, "Show"),
            WorkflowCommand::Create { .. } => assert_eq!(name, "Create"),
            WorkflowCommand::Cancel { .. } => assert_eq!(name, "Cancel"),
            WorkflowCommand::Dependencies { .. } => assert_eq!(name, "Dependencies"),
            WorkflowCommand::Graph { .. } => assert_eq!(name, "Graph"),
        }
    }
}

#[test]
fn test_complex_dependency_graph() {
    // Create a more complex dependency graph for testing
    let complex_jobs = vec![
        // Root jobs (no dependencies)
        JobNode {
            id: "root1".to_string(),
            queue_name: "init".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec!["child1".to_string(), "child2".to_string()],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
        JobNode {
            id: "root2".to_string(),
            queue_name: "init".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec!["child3".to_string()],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
        // Middle tier jobs
        JobNode {
            id: "child1".to_string(),
            queue_name: "process".to_string(),
            status: "Running".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["root1".to_string()],
            dependents: vec!["grandchild1".to_string()],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
        JobNode {
            id: "child2".to_string(),
            queue_name: "process".to_string(),
            status: "Pending".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["root1".to_string()],
            dependents: vec!["grandchild2".to_string()],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
        JobNode {
            id: "child3".to_string(),
            queue_name: "process".to_string(),
            status: "Failed".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["root2".to_string()],
            dependents: vec!["grandchild2".to_string()],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
        // Leaf jobs (jobs that depend on multiple parents)
        JobNode {
            id: "grandchild1".to_string(),
            queue_name: "finalize".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["child1".to_string()],
            dependents: vec![],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
        JobNode {
            id: "grandchild2".to_string(),
            queue_name: "finalize".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["child2".to_string(), "child3".to_string()],
            dependents: vec![],
            workflow_id: Some("complex-workflow".to_string()),
            workflow_name: Some("complex-test".to_string()),
        },
    ];

    let cmd = create_test_workflow_command();
    let levels = cmd.calculate_dependency_levels(&complex_jobs);

    // Should have at least 3 levels for this complex graph
    assert!(levels.len() >= 3);

    // Verify root jobs are at level 0
    let level_0_jobs = levels
        .iter()
        .find(|(level, _)| *level == 0)
        .map(|(_, jobs)| jobs)
        .unwrap();

    let level_0_ids: HashSet<String> = level_0_jobs.iter().map(|job| job.id.clone()).collect();

    assert!(level_0_ids.contains("root1"));
    assert!(level_0_ids.contains("root2"));
}

#[test]
fn test_disconnected_job_handling() {
    // Test jobs that are not connected to the main dependency graph
    let disconnected_jobs = vec![
        JobNode {
            id: "isolated1".to_string(),
            queue_name: "isolated".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("isolated-workflow".to_string()),
            workflow_name: Some("isolated-test".to_string()),
        },
        JobNode {
            id: "isolated2".to_string(),
            queue_name: "isolated".to_string(),
            status: "Pending".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("isolated-workflow".to_string()),
            workflow_name: Some("isolated-test".to_string()),
        },
    ];

    let cmd = create_test_workflow_command();
    let levels = cmd.calculate_dependency_levels(&disconnected_jobs);

    // All disconnected jobs should be at level 0
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].0, 0);
    assert_eq!(levels[0].1.len(), 2);
}

#[test]
fn test_cyclic_dependency_detection() {
    // This test helps ensure our algorithms can handle potentially cyclic data
    // (though cycles shouldn't exist in a properly validated workflow)
    let potentially_cyclic_jobs = vec![
        JobNode {
            id: "job_a".to_string(),
            queue_name: "test".to_string(),
            status: "Running".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["job_c".to_string()],
            dependents: vec!["job_b".to_string()],
            workflow_id: Some("cyclic-test".to_string()),
            workflow_name: Some("cyclic".to_string()),
        },
        JobNode {
            id: "job_b".to_string(),
            queue_name: "test".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["job_a".to_string()],
            dependents: vec!["job_c".to_string()],
            workflow_id: Some("cyclic-test".to_string()),
            workflow_name: Some("cyclic".to_string()),
        },
        JobNode {
            id: "job_c".to_string(),
            queue_name: "test".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["job_b".to_string()],
            dependents: vec!["job_a".to_string()],
            workflow_id: Some("cyclic-test".to_string()),
            workflow_name: Some("cyclic".to_string()),
        },
    ];

    let cmd = create_test_workflow_command();

    // Our algorithm should handle this gracefully without infinite loops
    let levels = cmd.calculate_dependency_levels(&potentially_cyclic_jobs);

    // Should produce some levels without crashing
    assert!(!levels.is_empty());

    // All jobs should be accounted for
    let total_jobs_in_levels: usize = levels.iter().map(|(_, jobs)| jobs.len()).sum();
    assert_eq!(total_jobs_in_levels, potentially_cyclic_jobs.len());
}

#[test]
fn test_uuid_parsing_edge_cases() {
    let valid_uuids = vec![
        "550e8400-e29b-41d4-a716-446655440000",
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        "6ba7b811-9dad-11d1-80b4-00c04fd430c8",
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ];

    for uuid_str in valid_uuids {
        let parsed = Uuid::parse_str(uuid_str);
        assert!(parsed.is_ok(), "Failed to parse valid UUID: {}", uuid_str);
    }

    let invalid_uuids = vec![
        "not-a-uuid",
        "550e8400-e29b-41d4-a716",
        "550e8400-e29b-41d4-a716-446655440000-extra",
        "",
        "550e8400-e29b-41d4-a716-44665544000g", // invalid character
    ];

    for uuid_str in invalid_uuids {
        let parsed = Uuid::parse_str(uuid_str);
        assert!(
            parsed.is_err(),
            "Should have failed to parse invalid UUID: {}",
            uuid_str
        );
    }
}

#[test]
fn test_job_node_edge_cases() {
    // Test job with no workflow
    let orphaned_job = JobNode {
        id: "orphan".to_string(),
        queue_name: "orphan-queue".to_string(),
        status: "Pending".to_string(),
        dependency_status: "none".to_string(),
        depends_on: vec![],
        dependents: vec![],
        workflow_id: None,
        workflow_name: None,
    };

    assert!(orphaned_job.workflow_id.is_none());
    assert!(orphaned_job.workflow_name.is_none());

    // Test job with very long dependency lists
    let many_deps: Vec<String> = (0..100).map(|i| format!("dep-{:03}", i)).collect();

    let many_dependents: Vec<String> = (0..50).map(|i| format!("dependent-{:03}", i)).collect();

    let busy_job = JobNode {
        id: "busy-job".to_string(),
        queue_name: "busy-queue".to_string(),
        status: "Running".to_string(),
        dependency_status: "satisfied".to_string(),
        depends_on: many_deps.clone(),
        dependents: many_dependents.clone(),
        workflow_id: Some("busy-workflow".to_string()),
        workflow_name: Some("busy-test".to_string()),
    };

    assert_eq!(busy_job.depends_on.len(), 100);
    assert_eq!(busy_job.dependents.len(), 50);
}
