# Changelog

## Unreleased
- A repeating task without a date (`🔁 every 2 weeks` and no `📅`/`🛫`) reaches the server. Radicale refused it (`HTTP 400`, "push failed; the next pass retries" at every pass). Its `VTODO` now carries a start date marked as restask's anchor; the line in the note gets no date. Other apps show that date as the task's start.
- `restask setup` tries the task server from the always-on server before it changes the vault. A server URL with a name that only the home network's DNS knows no longer ends in "the daemon could not join the vault … error sending request": the daemon is given the address the name has on the computer setup runs on. A server that cannot be reached from there stops the run at once, with the reason.
- Several vaults on one computer and one server. Each vault has its own machine config (`~/.config/restask/config.toml` for the first, `vaults/<vault folder>/config.toml` for a further one), its own daemon unit and, on the server, its own stack (`restask`, then `restask-<vault folder>`, each with its own container and image). `restask setup` for a second vault no longer stops at "the stack in restask serves another vault", and no longer overwrites the first vault's config. A command loads the config of the vault it is run in.
- First public preparation: contributor docs, security policy, issue and PR templates, release workflow.
