# ADR-0006: Profile Match Precedence

## Context

OQ-015 asked which profile wins when multiple loaded profiles match the same
foreground window. M3 profile resolution must be deterministic across sessions
and explainable in `profile_list`, `profile_activate`, and manual FSV evidence.

## Decision

Profile resolver precedence is based on the strongest matched field:

1. `exe`
2. `title_regex`
3. `steam_appid`
4. `window_class`

Each profile may contain multiple `[[matches]]` entries; the resolver uses the
strongest matching field from that profile. Across profiles with the same
strongest field, the newer profile file mtime wins. Remaining exact ties are
broken deterministically by source path, profile id, and loaded index.

Manual `profile_activate(profile_id=...)` is an explicit operator/agent
override and sets the active profile directly. Automatic foreground resolution
does not silently override a manual activation unless the caller invokes the
foreground resolver again.

## Rationale

Executable identity is the least ambiguous foreground signal, followed by title
regex, Steam app id, and window class. Newer file mtime gives operator-edited
profiles a predictable way to override same-rank bundled behavior without
depending on filesystem iteration order.

## Alternatives Considered

- First loaded profile wins - rejected because loaded order changes across
  directories and machines.
- User-installed directory always wins - rejected for M3 because the runtime has
  one active profile directory at a time, and field specificity is a clearer
  conflict resolver.
- Full weighted scoring - rejected because the current four-field rank covers
  M3 ambiguity without adding hard-to-debug weights.

## Consequences

- Positive: profile matches are deterministic and explainable from
  `rank_name`, profile file mtimes, and file paths.
- Positive: same-rank operator edits can override bundled/default profiles by
  producing a newer mtime.
- Negative: a broad `exe` match beats a narrow `title_regex` match in another
  profile.
- Trade-off accepted: callers can use `profile_activate` for explicit override
  when field-rank precedence is not what they want.

## Supersedes

- OQ-015 in `docs/computergames/16_open_questions.md`

## References

- Decision issue: #338
- Resolver: `crates/synapse-profiles/src/resolver.rs`
- Profile docs: `docs/computergames/07_storage_and_profiles.md` §8.3
