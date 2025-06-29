use cargo_hammerwork::commands::workflow::{WorkflowCommand, JobNode};

/// Create a simple workflow command for testing output formats
fn create_test_command() -> WorkflowCommand {
    WorkflowCommand::Graph {
        database_url: None,
        workflow_id: "test-workflow-123".to_string(),
        format: Some("text".to_string()),
    }
}

/// Create sample jobs for format testing
fn create_sample_jobs() -> Vec<JobNode> {
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
            dependents: vec!["abcdef12-abcd-abcd-abcd-abcdef123456".to_string(), "fedcba98-fedc-fedc-fedc-fedcba987654".to_string()],
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

#[test]
fn test_text_graph_format_structure() {
    let cmd = create_test_command();
    let jobs = create_sample_jobs();
    
    // Test that we can calculate dependency levels without errors
    let levels = cmd.calculate_dependency_levels(&jobs);
    
    // Should have multiple levels for our sample data
    assert!(!levels.is_empty());
    assert!(levels.len() >= 2); // At least root and one dependent level
    
    // Verify level 0 contains root jobs
    let level_0_jobs = levels.iter()
        .find(|(level, _)| *level == 0)
        .map(|(_, jobs)| jobs);
    
    assert!(level_0_jobs.is_some());
    let level_0 = level_0_jobs.unwrap();
    
    // Root job should be at level 0
    assert!(level_0.iter().any(|job| job.depends_on.is_empty()));
}

#[test]
fn test_json_graph_format_structure() {
    let cmd = create_test_command();
    let jobs = create_sample_jobs();
    
    // Test the JSON format generation by verifying the data structure
    let result = cmd.print_json_graph(&jobs);
    assert!(result.is_ok());
    
    // Verify job data structure for JSON serialization
    for job in &jobs {
        // Ensure all required fields exist
        assert!(!job.id.is_empty());
        assert!(!job.queue_name.is_empty());
        assert!(!job.status.is_empty());
        assert!(!job.dependency_status.is_empty());
        
        // Verify dependency arrays are valid
        for dep_id in &job.depends_on {
            assert!(!dep_id.is_empty());
        }
        
        for dep_id in &job.dependents {
            assert!(!dep_id.is_empty());
        }
    }
}

#[test]
fn test_dependency_graph_relationships() {
    let jobs = create_sample_jobs();
    
    // Build relationships map for testing
    use std::collections::HashMap;
    let job_map: HashMap<String, &JobNode> = jobs.iter()
        .map(|job| (job.id.clone(), job))
        .collect();
    
    // Test forward relationships (depends_on -> dependents)
    for job in &jobs {
        for dep_id in &job.depends_on {
            if let Some(dependency) = job_map.get(dep_id) {
                // The dependency should list this job as a dependent
                assert!(
                    dependency.dependents.contains(&job.id),
                    "Job {} depends on {}, but {} doesn't list it as dependent",
                    job.id, dep_id, dep_id
                );
            }
        }
    }
    
    // Test backward relationships (dependents -> depends_on)
    for job in &jobs {
        for dependent_id in &job.dependents {
            if let Some(dependent) = job_map.get(dependent_id) {
                // The dependent should list this job as a dependency
                assert!(
                    dependent.depends_on.contains(&job.id),
                    "Job {} lists {} as dependent, but {} doesn't depend on it",
                    job.id, dependent_id, dependent_id
                );
            }
        }
    }
}

#[test]
fn test_status_and_dependency_status_consistency() {
    let jobs = create_sample_jobs();
    
    for job in &jobs {
        // Test status consistency
        match job.status.as_str() {
            "Completed" => {
                // Completed jobs should generally not be waiting
                assert_ne!(job.dependency_status, "waiting");
            }
            "Failed" => {
                // Failed jobs might have failed dependencies
                assert!(["failed", "satisfied", "none"].contains(&job.dependency_status.as_str()));
            }
            "Pending" => {
                // Pending jobs are often waiting for dependencies
                assert!(["waiting", "satisfied", "none"].contains(&job.dependency_status.as_str()));
            }
            "Running" => {
                // Running jobs should have satisfied dependencies
                assert!(["satisfied", "none"].contains(&job.dependency_status.as_str()));
            }
            _ => {
                // Other statuses should be valid
                assert!(["none", "waiting", "satisfied", "failed"].contains(&job.dependency_status.as_str()));
            }
        }
    }
}

#[test]
fn test_workflow_id_consistency() {
    let jobs = create_sample_jobs();
    
    // All jobs in our sample should belong to the same workflow
    let workflow_ids: std::collections::HashSet<Option<String>> = jobs.iter()
        .map(|job| job.workflow_id.clone())
        .collect();
    
    // Should have exactly one workflow ID (all jobs in same workflow)
    assert_eq!(workflow_ids.len(), 1);
    
    let workflow_id = jobs[0].workflow_id.as_ref().unwrap();
    for job in &jobs {
        assert_eq!(job.workflow_id.as_ref().unwrap(), workflow_id);
    }
}

#[test]
fn test_queue_name_variety() {
    let jobs = create_sample_jobs();
    
    // Collect all unique queue names
    let queue_names: std::collections::HashSet<String> = jobs.iter()
        .map(|job| job.queue_name.clone())
        .collect();
    
    // Should have multiple different queues
    assert!(queue_names.len() > 1);
    
    // Verify expected queue names exist
    assert!(queue_names.contains("data-processing"));
    assert!(queue_names.contains("data-transform"));
    assert!(queue_names.contains("data-export"));
    assert!(queue_names.contains("cleanup"));
}

#[test]
fn test_job_id_uniqueness() {
    let jobs = create_sample_jobs();
    
    // Collect all job IDs
    let job_ids: Vec<String> = jobs.iter()
        .map(|job| job.id.clone())
        .collect();
    
    // Convert to set to check uniqueness
    let unique_ids: std::collections::HashSet<String> = job_ids.iter().cloned().collect();
    
    // All IDs should be unique
    assert_eq!(job_ids.len(), unique_ids.len());
    
    // All IDs should be valid UUIDs
    for id in &job_ids {
        let uuid_result = uuid::Uuid::parse_str(id);
        assert!(uuid_result.is_ok(), "Invalid UUID format: {}", id);
    }
}

#[test]
fn test_dependency_level_algorithm_correctness() {
    let cmd = create_test_command();
    let jobs = create_sample_jobs();
    
    let levels = cmd.calculate_dependency_levels(&jobs);
    
    // Build a map for easier lookup
    use std::collections::HashMap;
    let mut job_to_level = HashMap::new();
    for (level, jobs_at_level) in &levels {
        for job in jobs_at_level {
            job_to_level.insert(&job.id, *level);
        }
    }
    
    for job in &jobs {
        let job_level = job_to_level.get(&job.id).unwrap();
        
        // All dependencies should be at a lower level
        for dep_id in &job.depends_on {
            if let Some(dep_level) = job_to_level.get(dep_id) {
                assert!(
                    dep_level < job_level,
                    "Job {} at level {} depends on job {} at level {} (should be lower)",
                    job.id, job_level, dep_id, dep_level
                );
            }
        }
    }
}

#[test]
fn test_empty_jobs_handling() {
    let cmd = create_test_command();
    let empty_jobs = vec![];
    
    // Should handle empty job list gracefully
    let levels = cmd.calculate_dependency_levels(&empty_jobs);
    assert!(levels.is_empty());
    
    // JSON output should handle empty jobs
    let result = cmd.print_json_graph(&empty_jobs);
    assert!(result.is_ok());
}

#[test]
fn test_single_job_handling() {
    let cmd = create_test_command();
    let single_job = vec![JobNode {
        id: "single-job".to_string(),
        queue_name: "solo-queue".to_string(),
        status: "Completed".to_string(),
        dependency_status: "none".to_string(),
        depends_on: vec![],
        dependents: vec![],
        workflow_id: Some("solo-workflow".to_string()),
        workflow_name: Some("solo".to_string()),
    }];
    
    // Should handle single job correctly
    let levels = cmd.calculate_dependency_levels(&single_job);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].0, 0); // Should be at level 0
    assert_eq!(levels[0].1.len(), 1); // Should contain the single job
    
    // JSON output should work
    let result = cmd.print_json_graph(&single_job);
    assert!(result.is_ok());
}

#[test]
fn test_parallel_jobs_same_level() {
    let cmd = create_test_command();
    
    // Create parallel jobs (no dependencies between them)
    let parallel_jobs = vec![
        JobNode {
            id: "parallel1".to_string(),
            queue_name: "parallel-queue".to_string(),
            status: "Running".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("parallel-workflow".to_string()),
            workflow_name: Some("parallel".to_string()),
        },
        JobNode {
            id: "parallel2".to_string(),
            queue_name: "parallel-queue".to_string(),
            status: "Pending".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("parallel-workflow".to_string()),
            workflow_name: Some("parallel".to_string()),
        },
        JobNode {
            id: "parallel3".to_string(),
            queue_name: "parallel-queue".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("parallel-workflow".to_string()),
            workflow_name: Some("parallel".to_string()),
        },
    ];
    
    let levels = cmd.calculate_dependency_levels(&parallel_jobs);
    
    // All parallel jobs should be at level 0
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].0, 0);
    assert_eq!(levels[0].1.len(), 3);
}

#[test]
fn test_deep_dependency_chain() {
    let cmd = create_test_command();
    
    // Create a deep dependency chain
    let chain_jobs = vec![
        JobNode {
            id: "chain0".to_string(),
            queue_name: "chain".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec!["chain1".to_string()],
            workflow_id: Some("chain-workflow".to_string()),
            workflow_name: Some("chain".to_string()),
        },
        JobNode {
            id: "chain1".to_string(),
            queue_name: "chain".to_string(),
            status: "Completed".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["chain0".to_string()],
            dependents: vec!["chain2".to_string()],
            workflow_id: Some("chain-workflow".to_string()),
            workflow_name: Some("chain".to_string()),
        },
        JobNode {
            id: "chain2".to_string(),
            queue_name: "chain".to_string(),
            status: "Running".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["chain1".to_string()],
            dependents: vec!["chain3".to_string()],
            workflow_id: Some("chain-workflow".to_string()),
            workflow_name: Some("chain".to_string()),
        },
        JobNode {
            id: "chain3".to_string(),
            queue_name: "chain".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["chain2".to_string()],
            dependents: vec![],
            workflow_id: Some("chain-workflow".to_string()),
            workflow_name: Some("chain".to_string()),
        },
    ];
    
    let levels = cmd.calculate_dependency_levels(&chain_jobs);
    
    // Should have 4 levels for 4 chained jobs
    assert_eq!(levels.len(), 4);
    
    // Verify each level has exactly one job
    for (level, jobs_at_level) in &levels {
        assert_eq!(jobs_at_level.len(), 1, "Level {} should have exactly 1 job", level);
    }
    
    // Verify levels are 0, 1, 2, 3
    let mut level_numbers: Vec<usize> = levels.iter().map(|(level, _)| *level).collect();
    level_numbers.sort();
    assert_eq!(level_numbers, vec![0, 1, 2, 3]);
}

#[test]
fn test_format_options_validation() {
    // Test all supported format options
    let supported_formats = vec!["text", "dot", "mermaid", "json"];
    
    for format in supported_formats {
        let cmd = WorkflowCommand::Graph {
            database_url: None,
            workflow_id: "test".to_string(),
            format: Some(format.to_string()),
        };
        
        // Should be able to create command with any supported format
        match cmd {
            WorkflowCommand::Graph { format: Some(f), .. } => {
                assert_eq!(f, format);
            }
            _ => panic!("Expected Graph command"),
        }
    }
}

#[test]
fn test_workflow_name_handling() {
    let jobs_with_names = vec![
        JobNode {
            id: "job1".to_string(),
            queue_name: "queue1".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("workflow1".to_string()),
            workflow_name: Some("My Workflow".to_string()),
        },
        JobNode {
            id: "job2".to_string(),
            queue_name: "queue2".to_string(),
            status: "Running".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec![],
            workflow_id: Some("workflow2".to_string()),
            workflow_name: None, // No name
        },
    ];
    
    // Test that jobs can have workflow names or not
    assert!(jobs_with_names[0].workflow_name.is_some());
    assert!(jobs_with_names[1].workflow_name.is_none());
    
    // Both should be valid
    for job in &jobs_with_names {
        assert!(!job.id.is_empty());
        assert!(job.workflow_id.is_some());
    }
}
