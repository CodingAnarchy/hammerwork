# Upgrading from 2.x to 3.0

This page lists what to change in your project when you move from Hammerwork 2.x to 3.0.

## Feature flags

Three changes to the Cargo features of `hammerwork` (#92):

1. **No default features.** In 2.x, `default = ["metrics", "alerting", "webhooks"]`. In 3.0, `default = []`, so a plain `hammerwork` dependency builds only the queue, worker and job APIs, without `warp`, `reqwest`, `lettre` or `prometheus`.
2. **`azure-kv` is now `azure-kms`**, like `aws-kms`, `gcp-kms` and `vault-kms`. There is no alias.
3. **`test` is now `test-queue`.** There is no alias.

New: a `full` feature that enables `metrics`, `alerting`, `webhooks`, `streaming`, `tracing` and `encryption`. It does not include a database backend (`postgres`, `mysql`), a KMS key source, an event stream sink (`kafka`, `google-pubsub`, `kinesis`) or `test-queue`.

### What to change

If your 2.x `Cargo.toml` relied on the defaults, name the features you use:

```toml
# 2.x (implicitly metrics + alerting + webhooks)
hammerwork = { version = "2", features = ["postgres"] }

# 3.0, same behaviour
hammerwork = { version = "3", features = ["postgres", "metrics", "alerting", "webhooks"] }

# 3.0, or everything that is not a backend, KMS source or event sink
hammerwork = { version = "3", features = ["postgres", "full"] }
```

If you used `default-features = false`, you can drop it.

Rename the other features:

```toml
# 2.x
hammerwork = { version = "2", features = ["postgres", "encryption", "azure-kv"] }
[dev-dependencies]
hammerwork = { version = "2", features = ["test"] }

# 3.0
hammerwork = { version = "3", features = ["postgres", "encryption", "azure-kms"] }
[dev-dependencies]
hammerwork = { version = "3", features = ["test-queue"] }
```

A compile error such as `unresolved import hammerwork::metrics`, `hammerwork::alerting` or `hammerwork::webhooks`, or a missing `TestQueue`, means a feature is not enabled. `cargo-hammerwork` and `hammerwork-web` already enable the features they need.

If you gate your own code with `cfg(feature = "...")` on features that you forward to hammerwork, rename them in the same way.
