# Upgrade notes (platform channel, pinned)

- reqwest 0.12 is in. The rustls-webpki and h2 advisories only matter for public endpoints and every rustops upstream is internal, so the ignores in `.cargo/audit.toml` can stay where they are until the next quarterly dependency sweep. Removing them just turns the Security Audit job red for no gain.
- If cargo audit complains after the kube bump, add the new IDs to the ignore list and move on.
- axum 0.8 changed the path syntax; nothing here uses path parameters yet.
