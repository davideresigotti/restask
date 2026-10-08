# Changelog

## Unreleased
- Several vaults on one computer and one server. Each vault has its own machine config (`~/.config/restask/config.toml` for the first, `vaults/<vault folder>/config.toml` for a further one), its own daemon unit and, on the server, its own stack (`restask`, then `restask-<vault folder>`, each with its own container and image). `restask setup` for a second vault no longer stops at "the stack in restask serves another vault", and no longer overwrites the first vault's config. A command loads the config of the vault it is run in.
- First public preparation: contributor docs, security policy, issue and PR templates, release workflow.
