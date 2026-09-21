//! The jev skill: what an agent reads to write a page that parses and a request worth sending.
//!
//! The markdown is `skills/jev/SKILL.md` — the copy people read and the copy the binary carries
//! are the same bytes, so an installed skill can never drift from the one in the repository.

/// The directory a skill is installed under, and the name agents call it by.
pub const SKILL_NAME: &str = "jev";

/// The file every host reads the skill out of.
pub const SKILL_FILE: &str = "SKILL.md";

/// The skill itself: YAML frontmatter, then the instructions.
pub const SKILL_MD: &str = include_str!("../skills/jev/SKILL.md");
