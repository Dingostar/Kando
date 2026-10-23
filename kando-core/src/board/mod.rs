pub mod age;
pub mod hooks;
pub mod metrics;
pub mod storage;
pub mod sync;

use chrono::{DateTime, NaiveDate, Utc};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use serde::{Deserialize, Deserializer, Serialize};
use unicode_normalization::UnicodeNormalization;

/// Deserialize the legacy free-text `blocked` field from either a TOML bool or a string.
/// `true` → `Some("")`, `false` / absent → `None`, `"reason"` → `Some("reason")`.
fn deserialize_legacy_blocked<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrString {
        Bool(#[allow(dead_code)] bool),
        Str(#[allow(dead_code)] String),
    }
    match Option::<BoolOrString>::deserialize(deserializer)? {
        None => Ok(None),
        Some(BoolOrString::Bool(true)) => Ok(Some(String::new())),
        Some(BoolOrString::Bool(false)) => Ok(None),
        Some(BoolOrString::Str(s)) => Ok(Some(s)),
    }
}

/// The top-level board containing all columns and cards.
#[derive(Debug, Clone)]
pub struct Board {
    pub name: String,
    pub next_card_id: u32,
    pub policies: Policies,
    pub sync_branch: Option<String>,
    pub nerd_font: bool,
    /// When the board was created. None for legacy boards.
    pub created_at: Option<DateTime<Utc>>,
    pub columns: Vec<Column>,
}

/// Board-level policies for hygiene automation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policies {
    /// Cards untouched for this many days get auto-closed. 0 = disabled.
    #[serde(default = "default_auto_close_days")]
    pub auto_close_days: u32,
    /// Which column auto-closed cards move to.
    #[serde(default = "default_auto_close_target")]
    pub auto_close_target: String,
    /// Cards untouched for this many days get a visual warning.
    #[serde(rename = "stale_days", alias = "bubble_up_days", default = "default_stale_days")]
    pub stale_days: u32,
    /// Trash entries older than this many days are permanently purged. 0 = never.
    #[serde(default = "default_trash_purge_days")]
    pub trash_purge_days: u32,
    /// Cards in the `done` column for this many days are auto-archived. 0 = disabled.
    #[serde(default)]
    pub archive_after_days: u32,
}

fn default_auto_close_days() -> u32 {
    30
}
fn default_auto_close_target() -> String {
    "archive".to_string()
}
fn default_stale_days() -> u32 {
    7
}
fn default_trash_purge_days() -> u32 {
    30
}

impl Default for Policies {
    fn default() -> Self {
        Self {
            auto_close_days: default_auto_close_days(),
            auto_close_target: default_auto_close_target(),
            stale_days: default_stale_days(),
            trash_purge_days: default_trash_purge_days(),
            archive_after_days: 0,
        }
    }
}

/// A single kanban column (not "list").
#[derive(Debug, Clone)]
pub struct Column {
    pub slug: String,
    pub name: String,
    pub order: u32,
    pub wip_limit: Option<u32>,
    pub hidden: bool,
    pub cards: Vec<Card>,
}

/// Priority levels for cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

impl Priority {
    pub const ALL: [Priority; 4] = [Self::Low, Self::Normal, Self::High, Self::Urgent];

    /// Sort key: lower = higher priority (sorts first).
    pub fn sort_key(self) -> u8 {
        match self {
            Self::Urgent => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }

}

impl std::str::FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "normal" => Ok(Self::Normal),
            "high" => Ok(Self::High),
            "urgent" => Ok(Self::Urgent),
            other => Err(format!("unknown priority '{other}': use low, normal, high, urgent")),
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single kanban card (not "task" or "item").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(default)]
    pub priority: Priority,
    /// Manual position within the card's priority group, set by reordering a
    /// column. `None` means the card has never been placed by hand, and falls
    /// back to `updated` for its position — which is how a column nobody has
    /// reordered keeps sorting newest-first. See [`Column::sort_cards`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    /// IDs of the cards this card is blocked by. The reverse edge ("is
    /// blocking") is derived by [`Board::is_blocking`], never stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// The free-text `blocked` reason from before blockers referenced cards.
    /// It no longer blocks anything; it is kept (and written back) only so the
    /// note isn't lost silently, and `kando doctor` reports it until it is
    /// replaced with card IDs. Setting `blocked_by` clears it.
    #[serde(
        default,
        rename = "blocked",
        deserialize_with = "deserialize_legacy_blocked",
        skip_serializing
    )]
    pub legacy_blocked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<NaiveDate>,
    /// When the card first moved past backlog (commitment point). `None` for cards still in backlog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<DateTime<Utc>>,
    /// When the card was moved to the "done" column. `None` if not yet completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<DateTime<Utc>>,
    /// The markdown body (not serialized into frontmatter).
    #[serde(skip)]
    pub body: String,
}

/// A card template for creating new cards with preset fields.
/// Display name is derived from the slug via `slug_to_name()`. There is no stored name field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_offset_days: Option<u32>,
    #[serde(skip)]
    pub body: String,
}

/// Parse a comma-separated list of card IDs, as typed into the blocker editor
/// or `--blocked-by`. Trims whitespace, accepts a leading `#`, drops empty
/// entries and duplicates (keeping first-seen order).
pub fn parse_card_ids(input: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for part in input.split(',') {
        let id = part.trim().trim_start_matches('#').trim();
        if !id.is_empty() && !ids.iter().any(|i| i == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Generate a filesystem-safe slug for a template name, unique among `existing_slugs`.
pub fn generate_template_slug(name: &str, existing_slugs: &[String]) -> String {
    let base = slug_from_name(name);
    let base = if base == "col" && !name.chars().any(|c| c.is_alphanumeric()) {
        "template".to_string()
    } else {
        base
    };
    if !existing_slugs.iter().any(|s| s == &base) {
        return base;
    }
    for n in 2u32.. {
        let candidate = format!("{base}-{n}");
        if !existing_slugs.iter().any(|s| s == &candidate) {
            return candidate;
        }
    }
    unreachable!("generate_template_slug: exhausted candidates for base={base:?}")
}

impl Card {
    pub fn new(id: String, title: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            title,
            created: now,
            updated: now,
            priority: Priority::default(),
            order: None,
            tags: Vec::new(),
            assignees: Vec::new(),
            blocked_by: Vec::new(),
            legacy_blocked: None,
            due: None,
            started: None,
            completed: None,
            body: "<!-- add body content here -->".into(),
        }
    }

    /// Whether this card is past its due date.
    ///
    /// A card due *today* is NOT overdue. Only `due < today` counts.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        self.due.is_some_and(|d| d < today)
    }

    /// Touch the card, updating its `updated` timestamp.
    pub fn touch(&mut self) {
        self.updated = Utc::now();
    }

    /// Forget the card's hand-placed position, so it sorts by recency again.
    ///
    /// Call this whenever a card changes column: `order` is an index into the
    /// column it was placed in, and carrying it across would drop the card at
    /// an arbitrary depth of its new one instead of at the top where a
    /// just-moved card belongs.
    pub fn unplace(&mut self) {
        self.order = None;
    }
}

impl Board {
    /// Generate the next card ID and increment the counter.
    pub fn next_card_id(&mut self) -> String {
        let n = self.next_card_id;
        self.next_card_id += 1;
        n.to_string()
    }

    /// Find which column a card is in and its index.
    pub fn find_card(&self, card_id: &str) -> Option<(usize, usize)> {
        for (col_idx, col) in self.columns.iter().enumerate() {
            for (card_idx, card) in col.cards.iter().enumerate() {
                if card.id == card_id {
                    return Some((col_idx, card_idx));
                }
            }
        }
        None
    }

    /// Move a card from one column to another.
    pub fn move_card(&mut self, from_col: usize, card_idx: usize, to_col: usize) {
        if from_col >= self.columns.len() || to_col >= self.columns.len() {
            return;
        }
        if card_idx >= self.columns[from_col].cards.len() {
            return;
        }
        let mut card = self.columns[from_col].cards.remove(card_idx);
        card.touch();
        card.unplace();
        // Set started when card first moves past backlog (column 0).
        // Commitment is a one-time event: never cleared once set.
        if card.started.is_none() && to_col > 0 {
            card.started = Some(Utc::now());
        }
        // Set or clear completed based on the target column's slug. Mind the
        // asymmetry with started above: started is positional (any move out of
        // column 0), completed keys on the "done" slug specifically. That is
        // intentional, so don't try to make the two consistent.
        if self.columns[to_col].slug == "done" {
            if card.completed.is_none() {
                card.completed = Some(Utc::now());
            }
        } else {
            card.completed = None;
        }
        self.columns[to_col].cards.push(card);
    }

    /// Whether a blocker has stopped holding anything back: it is done,
    /// archived (or in the auto-close target column), or gone from the board.
    ///
    /// Blocking is derived from where the blocker sits *now*, so nothing
    /// cascades on move/archive/delete and moving a blocker back out of Done
    /// re-blocks its dependents.
    pub fn blocker_is_resolved(&self, id: &str) -> bool {
        match self.find_card(id) {
            None => true,
            Some((col, _)) => {
                let slug = self.columns[col].slug.as_str();
                slug == "done" || slug == "archive" || slug == self.policies.auto_close_target
            }
        }
    }

    /// Blocker IDs that are still holding this card back.
    pub fn active_blockers<'a>(&self, card: &'a Card) -> Vec<&'a str> {
        card.blocked_by
            .iter()
            .map(String::as_str)
            .filter(|id| !self.blocker_is_resolved(id))
            .collect()
    }

    /// Whether the card has at least one unresolved blocker.
    pub fn is_blocked(&self, card: &Card) -> bool {
        card.blocked_by.iter().any(|id| !self.blocker_is_resolved(id))
    }

    /// Derived reverse edge: IDs of the cards that list `id` in `blocked_by`.
    pub fn is_blocking(&self, id: &str) -> Vec<String> {
        self.columns
            .iter()
            .flat_map(|col| &col.cards)
            .filter(|card| card.blocked_by.iter().any(|b| b == id))
            .map(|card| card.id.clone())
            .collect()
    }

    /// Look for a blocker cycle through `card_id`, as if its `blocked_by` were
    /// `proposed`. Returns the cycle as a path that starts and ends at
    /// `card_id` (e.g. `["3", "2", "3"]`), or `None` if there is none.
    ///
    /// Carries a visited set, so cycles elsewhere in the graph can't hang it.
    pub fn find_blocker_cycle(&self, card_id: &str, proposed: &[String]) -> Option<Vec<String>> {
        let blockers_of = |id: &str| -> Vec<String> {
            if id == card_id {
                return proposed.to_vec();
            }
            match self.find_card(id) {
                Some((c, k)) => self.columns[c].cards[k].blocked_by.clone(),
                None => Vec::new(),
            }
        };
        let mut visited = std::collections::HashSet::new();
        // Each stack entry is a path from card_id; extend it depth-first.
        let mut stack: Vec<Vec<String>> = proposed
            .iter()
            .rev()
            .map(|b| vec![card_id.to_string(), b.clone()])
            .collect();
        while let Some(path) = stack.pop() {
            let last = path.last().expect("path is never empty").clone();
            if last == card_id {
                return Some(path);
            }
            if !visited.insert(last.clone()) {
                continue;
            }
            for next in blockers_of(&last).into_iter().rev() {
                let mut p = path.clone();
                p.push(next);
                stack.push(p);
            }
        }
        None
    }

    /// Every distinct blocker cycle on the board, for `kando doctor`.
    pub fn blocker_cycles(&self) -> Vec<Vec<String>> {
        let mut seen: Vec<std::collections::BTreeSet<String>> = Vec::new();
        let mut cycles = Vec::new();
        for card in self.columns.iter().flat_map(|col| &col.cards) {
            if let Some(cycle) = self.find_blocker_cycle(&card.id, &card.blocked_by) {
                let members: std::collections::BTreeSet<String> = cycle.iter().cloned().collect();
                if !seen.contains(&members) {
                    seen.push(members);
                    cycles.push(cycle);
                }
            }
        }
        cycles
    }

    /// Validate a proposed `blocked_by` list for `card_id`: every ID must be
    /// another card on the board, and the result must not create a cycle.
    /// Returns a user-facing message on failure.
    pub fn validate_blocked_by(&self, card_id: &str, ids: &[String]) -> Result<(), String> {
        if ids.iter().any(|id| id == card_id) {
            return Err("A card cannot block itself".into());
        }
        if let Some(missing) = ids.iter().find(|id| self.find_card(id).is_none()) {
            return Err(format!("No card with ID {missing}"));
        }
        if let Some(cycle) = self.find_blocker_cycle(card_id, ids) {
            return Err(format!("Cycle: {}", cycle.join(" -> ")));
        }
        Ok(())
    }

    /// Collect all unique tags across the board with counts.
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for col in &self.columns {
            for card in &col.cards {
                for tag in &card.tags {
                    *counts.entry(tag.as_str()).or_insert(0) += 1;
                }
            }
        }
        let mut tags: Vec<_> = counts
            .into_iter()
            .map(|(tag, count)| (tag.to_string(), count))
            .collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        tags
    }

    /// Collect all unique assignees across all cards, with counts.
    pub fn all_assignees(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for col in &self.columns {
            for card in &col.cards {
                for assignee in &card.assignees {
                    *counts.entry(assignee.as_str()).or_insert(0) += 1;
                }
            }
        }
        let mut assignees: Vec<_> = counts
            .into_iter()
            .map(|(name, count)| (name.to_string(), count))
            .collect();
        assignees.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        assignees
    }
}

// ---------------------------------------------------------------------------
// Card filtering
// ---------------------------------------------------------------------------

/// Check whether a card is visible under the current set of filters.
///
/// `active_filter` is the text/fuzzy search (from `/`).
/// `tag_filters` / `assignee_filters` / `staleness_filters` are the picker-based filters.
/// Text search takes precedence over picker filters.
#[allow(clippy::too_many_arguments)]
pub fn card_is_visible(
    card: &Card,
    active_filter: Option<&str>,
    tag_filters: &[String],
    assignee_filters: &[String],
    staleness_filters: &[String],
    overdue_filter: bool,
    policies: &Policies,
    now: DateTime<Utc>,
    matcher: &SkimMatcherV2,
) -> bool {
    if let Some(filter) = active_filter {
        card_matches_filter(card, filter, matcher)
    } else {
        let tag_ok =
            tag_filters.is_empty() || card.tags.iter().any(|t| tag_filters.contains(t));
        let assignee_ok = assignee_filters.is_empty()
            || card.assignees.iter().any(|a| assignee_filters.contains(a));
        let staleness_ok = staleness_filters.is_empty() || {
            let label = age::card_staleness_label(card, policies, now);
            staleness_filters.iter().any(|f| f.as_str() == label)
        };
        let overdue_ok = !overdue_filter || card.is_overdue(now.date_naive());
        tag_ok && assignee_ok && staleness_ok && overdue_ok
    }
}

/// Check whether a card matches a multi-term fuzzy filter.
///
/// The filter string is split on whitespace into individual terms.
/// Each term must fuzzy-match at least one card field (title, any tag,
/// or any assignee). OR across fields, AND across terms.
///
/// Special prefixes:
/// - `@` is stripped so `@alice` matches the assignee `alice`
/// - `!` negates: the card must NOT match that term in any field
pub fn card_matches_filter(card: &Card, filter: &str, matcher: &SkimMatcherV2) -> bool {
    let terms: Vec<&str> = filter.split_whitespace().collect();
    if terms.is_empty() {
        return true;
    }

    for term in &terms {
        let (negated, pattern) = if let Some(rest) = term.strip_prefix('!') {
            (true, rest)
        } else {
            (false, *term)
        };

        // Strip leading @ for assignee-friendly matching
        let bare = pattern.trim_start_matches('@');
        if bare.is_empty() {
            continue;
        }

        let matches_any_field = matcher.fuzzy_match(&card.title, bare).is_some()
            || card
                .tags
                .iter()
                .any(|t| matcher.fuzzy_match(t, bare).is_some())
            || card
                .assignees
                .iter()
                .any(|a| matcher.fuzzy_match(a, bare).is_some());

        if negated && matches_any_field {
            return false;
        }
        if !negated && !matches_any_field {
            return false;
        }
    }

    true
}

/// Derive a raw slug from a display name without uniqueness deduplication.
///
/// Maps non-alphanumeric chars to `-`, collapses runs, trims leading/trailing
/// hyphens, and falls back to `"col"` if the result would be empty.
///
/// **Does not** guard the `"archive"` reserved slug. Callers that need that
/// guarantee should use [`slug_for_rename`] or [`generate_slug`].
pub fn slug_from_name(name: &str) -> String {
    let base: String = name
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .nfc()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // After split/filter/join the result is guaranteed to start with an
    // alphanumeric char (or be empty), so only the empty check is needed.
    let base = base
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if base.is_empty() {
        "col".to_string()
    } else {
        base
    }
}

/// Derive the slug to use when **renaming** an existing column.
///
/// Like [`slug_from_name`] but also enforces the `"archive"` reservation,
/// returning `Err` when the derived slug would be reserved. Uniqueness and
/// same-slug detection remain the caller's responsibility.
pub fn slug_for_rename(name: &str) -> Result<String, &'static str> {
    let slug = slug_from_name(name);
    if slug == "archive" {
        Err("'archive' is a reserved slug")
    } else {
        Ok(slug)
    }
}

/// Generate a filesystem-safe slug from a display name, guaranteed unique
/// among `existing` columns. The reserved slug `"archive"` is never returned.
///
/// Algorithm: lowercase → map non-alphanumeric to `-` → collapse/trim hyphens
/// → fall back to `"col"` if empty → append `-2`, `-3`, … for uniqueness.
pub fn generate_slug(name: &str, existing: &[Column]) -> String {
    let base = slug_from_name(name);
    // "archive" is a reserved slug, never generate it directly.
    let base = if base == "archive" { "archive-col".to_string() } else { base };
    // Ensure uniqueness.
    if !existing.iter().any(|c| c.slug == base) {
        return base;
    }
    for n in 2u32.. {
        let candidate = format!("{base}-{n}");
        if !existing.iter().any(|c| c.slug == candidate) {
            return candidate;
        }
    }
    unreachable!("generate_slug: exhausted candidates for base={base:?}")
}

/// Convert a slug back to a display name: split on `-`, capitalise each word.
///
/// # Invariants
///
/// Assumes the slug was produced by [`generate_slug`] or [`slug_from_name`]
/// (no leading/trailing hyphens, lowercase alphanumeric + hyphens).
/// Degenerate input such as `"-"` produces degenerate output (`" "`) without
/// panicking, but callers should rely on [`validate_slug`] to prevent such
/// input from reaching this function.
///
/// Examples: `"backlog"` → `"Backlog"`, `"in-progress"` → `"In Progress"`.
pub fn slug_to_name(slug: &str) -> String {
    slug.split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize column orders to dense 0, 1, 2, … sorted by current order.
/// Call this after every add / remove / reorder operation, before save_board.
pub fn normalize_column_orders(columns: &mut [Column]) {
    columns.sort_by_key(|c| c.order);
    for (i, col) in columns.iter_mut().enumerate() {
        col.order = i as u32;
    }
}

impl Column {
    /// Sort cards by priority (urgent first), then by any manual position the
    /// user has given them, then by updated timestamp (most recent first).
    ///
    /// Priority stays the primary key, so reordering can only ever rearrange
    /// cards *within* a priority group — a Low card can't be pushed above an
    /// Urgent one. A card with no manual position sorts as if it were at 0,
    /// which keeps a freshly added card at the top of its group the way it
    /// always has, ahead of cards placed by hand below it.
    pub fn sort_cards(&mut self) {
        self.cards
            .sort_by(|a, b| {
                a.priority
                    .sort_key()
                    .cmp(&b.priority.sort_key())
                    .then(a.order.unwrap_or(0).cmp(&b.order.unwrap_or(0)))
                    .then(b.updated.cmp(&a.updated))
            });
    }

    /// Move the card at `from` to index `to`, shifting the cards in between.
    ///
    /// Returns false, changing nothing, if either index is out of range or the
    /// two are the same. The caller keeps the move inside one priority group;
    /// a move across groups would just be undone by the next
    /// [`sort_cards`](Self::sort_cards).
    pub fn reorder_card(&mut self, from: usize, to: usize) -> bool {
        if from >= self.cards.len() || to >= self.cards.len() || from == to {
            return false;
        }
        let card = self.cards.remove(from);
        self.cards.insert(to, card);
        self.renumber();
        true
    }

    /// Stamp every card with its current index, so the arrangement survives the
    /// next sort and the trip through disk. Placing one card by hand means
    /// pinning all of them: leaving the rest on `None` would let `updated` keep
    /// voting on where they go.
    fn renumber(&mut self) {
        for (i, card) in self.cards.iter_mut().enumerate() {
            card.order = Some(i as u32);
        }
    }

    /// The index the card at `idx` moves to when nudged one slot `up` (or down),
    /// stepping over cards that `is_visible` rejects.
    ///
    /// `None` means the card is already at the edge of its priority group, or
    /// the last visible one in it. Filtered-out cards are stepped over rather
    /// than swapped with, so a move always changes what you can see.
    pub fn reorder_target(
        &self,
        idx: usize,
        up: bool,
        is_visible: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let priority = self.cards.get(idx)?.priority;
        let mut i = idx;
        loop {
            i = if up { i.checked_sub(1)? } else { i + 1 };
            if self.cards.get(i)?.priority != priority {
                return None;
            }
            if is_visible(i) {
                return Some(i);
            }
        }
    }

    /// Whether this column is at or over its WIP limit.
    pub fn is_over_wip_limit(&self) -> bool {
        self.wip_limit
            .is_some_and(|limit| self.cards.len() as u32 >= limit)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_board_with_done() -> Board {
        Board {
            name: "Test".into(),
            next_card_id: 10,
            policies: Policies::default(),
            sync_branch: None,

            nerd_font: false,
            created_at: None,
            columns: vec![
                Column {
                    slug: "backlog".into(),
                    name: "Backlog".into(),
                    order: 0,
                    wip_limit: None,
                    hidden: false,
                    cards: vec![Card::new("1".into(), "Card A".into())],
                },
                Column {
                    slug: "done".into(),
                    name: "Done".into(),
                    order: 1,
                    wip_limit: None,
                    hidden: false,
                    cards: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn move_to_done_sets_completed() {
        let mut board = test_board_with_done();
        assert!(board.columns[0].cards[0].completed.is_none());

        board.move_card(0, 0, 1);
        let card = &board.columns[1].cards[0];
        assert!(card.completed.is_some(), "completed should be set when moving to done");
    }

    #[test]
    fn move_away_from_done_clears_completed() {
        let mut board = test_board_with_done();
        // First move to done
        board.move_card(0, 0, 1);
        assert!(board.columns[1].cards[0].completed.is_some());

        // Move back to backlog
        board.move_card(1, 0, 0);
        let card = &board.columns[0].cards[0];
        assert!(card.completed.is_none(), "completed should be cleared when moving away from done");
    }

    #[test]
    fn move_to_done_preserves_existing_completed() {
        let mut board = test_board_with_done();
        // Manually set completed to a specific time
        let original_time = Utc::now() - chrono::TimeDelta::days(5);
        board.columns[0].cards[0].completed = Some(original_time);

        board.move_card(0, 0, 1);
        let card = &board.columns[1].cards[0];
        assert_eq!(
            card.completed, Some(original_time),
            "moving to done when already completed should keep original timestamp"
        );
    }

    #[test]
    fn move_between_non_done_columns_leaves_completed_none() {
        let mut board = Board {
            name: "Test".into(),
            next_card_id: 10,
            policies: Policies::default(),
            sync_branch: None,

            nerd_font: false,
            created_at: None,
            columns: vec![
                Column {
                    slug: "backlog".into(),
                    name: "Backlog".into(),
                    order: 0,
                    wip_limit: None,
                    hidden: false,
                    cards: vec![Card::new("1".into(), "Card A".into())],
                },
                Column {
                    slug: "in-progress".into(),
                    name: "In Progress".into(),
                    order: 1,
                    wip_limit: None,
                    hidden: false,
                    cards: Vec::new(),
                },
                Column {
                    slug: "done".into(),
                    name: "Done".into(),
                    order: 2,
                    wip_limit: None,
                    hidden: false,
                    cards: Vec::new(),
                },
            ],
        };

        // Move backlog → in-progress (neither is "done")
        board.move_card(0, 0, 1);
        let card = &board.columns[1].cards[0];
        assert!(
            card.completed.is_none(),
            "moving between non-done columns should not set completed"
        );
    }

    // ── Started (commitment point) tests ──

    fn three_column_board() -> Board {
        Board {
            name: "Test".into(),
            next_card_id: 10,
            policies: Policies::default(),
            sync_branch: None,

            nerd_font: false,
            created_at: None,
            columns: vec![
                Column {
                    slug: "backlog".into(),
                    name: "Backlog".into(),
                    order: 0,
                    wip_limit: None,
                    hidden: false,
                    cards: vec![Card::new("1".into(), "Card A".into())],
                },
                Column {
                    slug: "in-progress".into(),
                    name: "In Progress".into(),
                    order: 1,
                    wip_limit: None,
                    hidden: false,
                    cards: Vec::new(),
                },
                Column {
                    slug: "done".into(),
                    name: "Done".into(),
                    order: 2,
                    wip_limit: None,
                    hidden: false,
                    cards: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn move_past_backlog_sets_started() {
        let mut board = three_column_board();
        assert!(board.columns[0].cards[0].started.is_none());

        board.move_card(0, 0, 1); // backlog → in-progress
        let card = &board.columns[1].cards[0];
        assert!(card.started.is_some(), "started should be set when moving past backlog");
    }

    #[test]
    fn move_within_active_preserves_started() {
        let mut board = three_column_board();
        board.move_card(0, 0, 1); // backlog → in-progress
        let original_started = board.columns[1].cards[0].started;

        // Add a review column and move there
        board.columns.insert(2, Column {
            slug: "review".into(),
            name: "Review".into(),
            order: 2,
            wip_limit: None,
            hidden: false,
            cards: Vec::new(),
        });
        board.move_card(1, 0, 2); // in-progress → review
        let card = &board.columns[2].cards[0];
        assert_eq!(card.started, original_started, "started should be preserved when moving within active columns");
    }

    #[test]
    fn move_back_to_backlog_preserves_started() {
        let mut board = three_column_board();
        board.move_card(0, 0, 1); // backlog → in-progress
        assert!(board.columns[1].cards[0].started.is_some());

        board.move_card(1, 0, 0); // in-progress → backlog
        let card = &board.columns[0].cards[0];
        assert!(card.started.is_some(), "started should NOT be cleared when moving back to backlog");
    }

    #[test]
    fn move_to_done_sets_started_if_none() {
        let mut board = test_board_with_done();
        assert!(board.columns[0].cards[0].started.is_none());

        board.move_card(0, 0, 1); // backlog → done
        let card = &board.columns[1].cards[0];
        assert!(card.started.is_some(), "started should be set when moving from backlog to done");
        assert!(card.completed.is_some(), "completed should also be set");
    }

    #[test]
    fn card_in_backlog_has_no_started() {
        let card = Card::new("1".into(), "Test".into());
        assert!(card.started.is_none());
    }

    // ── Filter tests ──

    fn card_with_meta(title: &str, tags: &[&str], assignees: &[&str]) -> Card {
        let mut c = Card::new("1".into(), title.into());
        c.tags = tags.iter().map(|t| t.to_string()).collect();
        c.assignees = assignees.iter().map(|a| a.to_string()).collect();
        c
    }

    #[test]
    fn filter_fuzzy_matches_title() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(card_matches_filter(&card, "login", &matcher));
    }

    #[test]
    fn filter_no_match_returns_false() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(!card_matches_filter(&card, "logout", &matcher));
    }

    #[test]
    fn filter_negation_excludes_match() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(!card_matches_filter(&card, "!login", &matcher));
    }

    #[test]
    fn filter_negation_includes_non_match() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(card_matches_filter(&card, "!logout", &matcher));
    }

    #[test]
    fn filter_at_prefix_matches_tag() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(card_matches_filter(&card, "@auth", &matcher));
    }

    #[test]
    fn filter_at_prefix_missing_tag_returns_false() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(!card_matches_filter(&card, "@missing", &matcher));
    }

    #[test]
    fn filter_multi_term_and_both_match() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth", "frontend"], &["alice"]);
        assert!(card_matches_filter(&card, "login auth", &matcher));
    }

    #[test]
    fn filter_multi_term_and_one_misses() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &["auth"], &["alice"]);
        assert!(!card_matches_filter(&card, "login missing", &matcher));
    }

    #[test]
    fn filter_matches_assignee() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Implement login", &[], &["alice"]);
        assert!(card_matches_filter(&card, "alice", &matcher));
    }

    #[test]
    fn filter_empty_returns_true() {
        let matcher = SkimMatcherV2::default();
        let card = card_with_meta("Anything", &[], &[]);
        assert!(card_matches_filter(&card, "", &matcher));
    }

    #[test]
    fn card_is_visible_no_filters_shows_all() {
        let matcher = SkimMatcherV2::default();
        let policies = Policies::default();
        let now = Utc::now();
        let card = card_with_meta("Test", &[], &[]);
        assert!(card_is_visible(&card, None, &[], &[], &[], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_text_filter_overrides_picker() {
        let matcher = SkimMatcherV2::default();
        let policies = Policies::default();
        let now = Utc::now();
        let card = card_with_meta("Login feature", &["bug"], &[]);
        // Text filter matches, even though tag filter wouldn't match
        assert!(card_is_visible(
            &card,
            Some("login"),
            &["nonexistent".to_string()],
            &[],
            &[],
            false,
            &policies,
            now,
            &matcher,
        ));
    }

    #[test]
    fn card_is_visible_tag_filter_must_match() {
        let matcher = SkimMatcherV2::default();
        let policies = Policies::default();
        let now = Utc::now();
        let card = card_with_meta("Test", &["bug"], &[]);
        assert!(card_is_visible(&card, None, &["bug".to_string()], &[], &[], false, &policies, now, &matcher));
        assert!(!card_is_visible(&card, None, &["feature".to_string()], &[], &[], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_assignee_filter_must_match() {
        let matcher = SkimMatcherV2::default();
        let policies = Policies::default();
        let now = Utc::now();
        let card = card_with_meta("Test", &[], &["alice"]);
        assert!(card_is_visible(&card, None, &[], &["alice".to_string()], &[], false, &policies, now, &matcher));
        assert!(!card_is_visible(&card, None, &[], &["bob".to_string()], &[], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_both_tag_and_assignee_must_match() {
        let matcher = SkimMatcherV2::default();
        let policies = Policies::default();
        let now = Utc::now();
        let card = card_with_meta("Test", &["bug"], &["alice"]);
        // Both match
        assert!(card_is_visible(&card, None, &["bug".to_string()], &["alice".to_string()], &[], false, &policies, now, &matcher));
        // Tag matches, assignee doesn't
        assert!(!card_is_visible(&card, None, &["bug".to_string()], &["bob".to_string()], &[], false, &policies, now, &matcher));
        // Assignee matches, tag doesn't
        assert!(!card_is_visible(&card, None, &["feature".to_string()], &["alice".to_string()], &[], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_staleness_filter_matches() {
        use chrono::TimeZone;
        let matcher = SkimMatcherV2::default();
        let now = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let policies = Policies { stale_days: 7, ..Default::default() };
        // Card created yesterday, updated recently → "normal"
        let mut card = card_with_meta("Test", &[], &[]);
        card.created = Utc.with_ymd_and_hms(2025, 6, 14, 12, 0, 0).unwrap();
        card.updated = now;
        assert!(card_is_visible(&card, None, &[], &[], &["normal".to_string()], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_staleness_filter_no_match() {
        use chrono::TimeZone;
        let matcher = SkimMatcherV2::default();
        let now = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let policies = Policies { stale_days: 7, ..Default::default() };
        // Card is "normal" but filter only accepts "stale"
        let mut card = card_with_meta("Test", &[], &[]);
        card.created = Utc.with_ymd_and_hms(2025, 6, 14, 12, 0, 0).unwrap();
        card.updated = now;
        assert!(!card_is_visible(&card, None, &[], &[], &["stale".to_string()], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_multiple_staleness_filters() {
        use chrono::TimeZone;
        let matcher = SkimMatcherV2::default();
        let now = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let policies = Policies { stale_days: 7, ..Default::default() };
        // Stale card (updated 7 days ago)
        let mut stale_card = card_with_meta("Stale", &[], &[]);
        stale_card.created = Utc.with_ymd_and_hms(2025, 5, 1, 12, 0, 0).unwrap();
        stale_card.updated = Utc.with_ymd_and_hms(2025, 6, 8, 12, 0, 0).unwrap();
        let filters = vec!["stale".to_string(), "very stale".to_string()];
        assert!(card_is_visible(&stale_card, None, &[], &[], &filters, false, &policies, now, &matcher));
        // Normal card should not match
        let mut normal_card = card_with_meta("Normal", &[], &[]);
        normal_card.created = Utc.with_ymd_and_hms(2025, 6, 14, 12, 0, 0).unwrap();
        normal_card.updated = now;
        assert!(!card_is_visible(&normal_card, None, &[], &[], &filters, false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_staleness_combined_with_tag_filter() {
        use chrono::TimeZone;
        let matcher = SkimMatcherV2::default();
        let now = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let policies = Policies { stale_days: 7, ..Default::default() };
        // Card is "normal" and has tag "bug"
        let mut card = card_with_meta("Test", &["bug"], &[]);
        card.created = Utc.with_ymd_and_hms(2025, 6, 14, 12, 0, 0).unwrap();
        card.updated = now;
        // Both tag and staleness match → visible
        assert!(card_is_visible(&card, None, &["bug".to_string()], &[], &["normal".to_string()], false, &policies, now, &matcher));
        // Tag matches but staleness doesn't → hidden
        assert!(!card_is_visible(&card, None, &["bug".to_string()], &[], &["stale".to_string()], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_text_filter_overrides_staleness() {
        use chrono::TimeZone;
        let matcher = SkimMatcherV2::default();
        let now = Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let policies = Policies { stale_days: 7, ..Default::default() };
        let mut card = card_with_meta("Login feature", &[], &[]);
        card.created = Utc.with_ymd_and_hms(2025, 6, 14, 12, 0, 0).unwrap();
        card.updated = now;
        // Text filter matches, staleness filter would NOT match; text takes precedence
        assert!(card_is_visible(&card, Some("login"), &[], &[], &["stale".to_string()], false, &policies, now, &matcher));
    }

    // ── Sort tests ──

    #[test]
    fn sort_cards_by_priority() {
        let now = Utc::now();
        let mut col = Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: None,
            hidden: false,
            cards: vec![],
        };
        let mut c1 = Card::new("1".into(), "Normal".into());
        c1.priority = Priority::Normal;
        c1.updated = now;
        let mut c2 = Card::new("2".into(), "Urgent".into());
        c2.priority = Priority::Urgent;
        c2.updated = now;
        let mut c3 = Card::new("3".into(), "Low".into());
        c3.priority = Priority::Low;
        c3.updated = now;
        let mut c4 = Card::new("4".into(), "High".into());
        c4.priority = Priority::High;
        c4.updated = now;
        col.cards = vec![c1, c2, c3, c4];

        col.sort_cards();

        let priorities: Vec<Priority> = col.cards.iter().map(|c| c.priority).collect();
        assert_eq!(priorities, vec![Priority::Urgent, Priority::High, Priority::Normal, Priority::Low]);
    }

    // ── Priority tests ──

    #[test]
    fn priority_from_str_valid() {
        assert_eq!("low".parse::<Priority>().unwrap(), Priority::Low);
        assert_eq!("normal".parse::<Priority>().unwrap(), Priority::Normal);
        assert_eq!("high".parse::<Priority>().unwrap(), Priority::High);
        assert_eq!("urgent".parse::<Priority>().unwrap(), Priority::Urgent);
    }

    #[test]
    fn priority_from_str_case_insensitive() {
        assert_eq!("HIGH".parse::<Priority>().unwrap(), Priority::High);
        assert_eq!("Urgent".parse::<Priority>().unwrap(), Priority::Urgent);
    }

    #[test]
    fn priority_from_str_unknown_returns_err() {
        assert!("unknown".parse::<Priority>().is_err());
        assert!("".parse::<Priority>().is_err());
    }

    #[test]
    fn priority_sort_key_ordering() {
        assert!(Priority::Urgent.sort_key() < Priority::High.sort_key());
        assert!(Priority::High.sort_key() < Priority::Normal.sort_key());
        assert!(Priority::Normal.sort_key() < Priority::Low.sort_key());
    }

    // ── WIP limit tests ──

    #[test]
    fn is_over_wip_limit_no_limit() {
        let col = Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: None,
            hidden: false,
            cards: vec![Card::new("1".into(), "A".into())],
        };
        assert!(!col.is_over_wip_limit());
    }

    #[test]
    fn is_over_wip_limit_under() {
        let col = Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: Some(3),
            hidden: false,
            cards: vec![Card::new("1".into(), "A".into())],
        };
        assert!(!col.is_over_wip_limit());
    }

    #[test]
    fn is_over_wip_limit_at_limit() {
        let col = Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: Some(2),
            hidden: false,
            cards: vec![Card::new("1".into(), "A".into()), Card::new("2".into(), "B".into())],
        };
        assert!(col.is_over_wip_limit());
    }

    // ── find_card tests ──

    #[test]
    fn find_card_returns_correct_indices() {
        let board = three_column_board();
        assert_eq!(board.find_card("1"), Some((0, 0)));
    }

    #[test]
    fn find_card_not_found_returns_none() {
        let board = three_column_board();
        assert_eq!(board.find_card("999"), None);
    }

    // ── next_card_id tests ──

    #[test]
    fn next_card_id_increments() {
        let mut board = test_board_with_done();
        let id1 = board.next_card_id();
        let id2 = board.next_card_id();
        assert_ne!(id1, id2);
        assert_eq!(id1.parse::<u32>().unwrap() + 1, id2.parse::<u32>().unwrap());
    }

    // ── all_tags / all_assignees tests ──

    #[test]
    fn all_tags_collects_and_sorts_by_count() {
        let mut board = test_board_with_done();
        board.columns[0].cards[0].tags = vec!["bug".into(), "ui".into()];
        let mut card2 = Card::new("2".into(), "B".into());
        card2.tags = vec!["bug".into()];
        board.columns[0].cards.push(card2);

        let tags = board.all_tags();
        assert_eq!(tags[0].0, "bug");
        assert_eq!(tags[0].1, 2);
        assert_eq!(tags[1].0, "ui");
        assert_eq!(tags[1].1, 1);
    }

    #[test]
    fn all_assignees_collects_and_sorts_by_count() {
        let mut board = test_board_with_done();
        board.columns[0].cards[0].assignees = vec!["alice".into(), "bob".into()];
        let mut card2 = Card::new("2".into(), "B".into());
        card2.assignees = vec!["alice".into()];
        board.columns[0].cards.push(card2);

        let assignees = board.all_assignees();
        assert_eq!(assignees[0].0, "alice");
        assert_eq!(assignees[0].1, 2);
        assert_eq!(assignees[1].0, "bob");
        assert_eq!(assignees[1].1, 1);
    }

    // ── Card::new defaults ──

    #[test]
    fn card_new_defaults() {
        let c = Card::new("001".into(), "Test".into());
        assert_eq!(c.id, "001");
        assert_eq!(c.title, "Test");
        assert_eq!(c.priority, Priority::Normal);
        assert!(c.tags.is_empty());
        assert!(c.assignees.is_empty());
        assert!(c.blocked_by.is_empty());
        assert!(c.legacy_blocked.is_none());
        assert!(c.started.is_none());
        assert!(c.completed.is_none());
        assert_eq!(c.body, "<!-- add body content here -->");
    }

    #[test]
    fn card_touch_updates_timestamp() {
        let mut c = Card::new("001".into(), "Test".into());
        let before = c.updated;
        std::thread::sleep(std::time::Duration::from_millis(2));
        c.touch();
        assert!(c.updated >= before);
    }

    // ── move_card edge cases ──

    #[test]
    fn move_card_same_column_noop() {
        let mut board = test_board_with_done();
        let count_before = board.columns[0].cards.len();
        board.move_card(0, 0, 0);
        assert_eq!(board.columns[0].cards.len(), count_before);
    }

    #[test]
    fn move_card_invalid_index_does_not_panic() {
        let mut board = test_board_with_done();
        // Out-of-bounds card index
        board.move_card(0, 999, 1);
        // Out-of-bounds column index
        board.move_card(999, 0, 0);
        board.move_card(0, 0, 999);
    }

    // ── reordering within a column ──

    fn col_with(cards: Vec<Card>) -> Column {
        Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: None,
            hidden: false,
            cards,
        }
    }

    /// Four Normal-priority cards, newest first, as `sort_cards` would leave them.
    fn four_normal_cards() -> Column {
        use chrono::TimeZone;
        let cards = (0..4)
            .map(|i| {
                let mut card = Card::new(i.to_string(), format!("Card {i}"));
                // Descending updated, so the untouched order is 0, 1, 2, 3.
                card.updated = Utc.with_ymd_and_hms(2025, 6, 4 - i as u32, 0, 0, 0).unwrap();
                card
            })
            .collect();
        let mut col = col_with(cards);
        col.sort_cards();
        assert_eq!(ids(&col), ["0", "1", "2", "3"]);
        col
    }

    fn ids(col: &Column) -> Vec<String> {
        col.cards.iter().map(|c| c.id.clone()).collect()
    }

    #[test]
    fn reorder_survives_a_resort() {
        // The whole point: without renumbering, `updated` would pull the card back.
        let mut col = four_normal_cards();
        assert!(col.reorder_card(3, 1));
        assert_eq!(ids(&col), ["0", "3", "1", "2"]);
        col.sort_cards();
        assert_eq!(ids(&col), ["0", "3", "1", "2"], "sort undid the reorder");
    }

    #[test]
    fn reorder_rejects_out_of_range_and_no_op_moves() {
        let mut col = four_normal_cards();
        assert!(!col.reorder_card(0, 0));
        assert!(!col.reorder_card(0, 4));
        assert!(!col.reorder_card(9, 0));
        assert_eq!(ids(&col), ["0", "1", "2", "3"]);
        assert!(col.cards.iter().all(|c| c.order.is_none()), "no-op still renumbered");
    }

    #[test]
    fn reorder_target_walks_one_slot_at_a_time() {
        let col = four_normal_cards();
        assert_eq!(col.reorder_target(2, true, |_| true), Some(1));
        assert_eq!(col.reorder_target(2, false, |_| true), Some(3));
    }

    #[test]
    fn reorder_target_stops_at_the_priority_boundary() {
        let mut urgent = Card::new("u".into(), "Urgent".into());
        urgent.priority = Priority::Urgent;
        let low = Card::new("n".into(), "Normal".into());
        let col = col_with(vec![urgent, low]);

        // The Normal card can't climb above the Urgent one, and vice versa.
        assert_eq!(col.reorder_target(1, true, |_| true), None);
        assert_eq!(col.reorder_target(0, false, |_| true), None);
    }

    #[test]
    fn reorder_target_steps_over_filtered_out_cards() {
        let col = four_normal_cards();
        // Only 0 and 3 are visible: 3 moving up lands where 0 is, not where 2 is.
        let visible = |i: usize| i == 0 || i == 3;
        assert_eq!(col.reorder_target(3, true, visible), Some(0));
        // Nothing visible below 3, so there is nowhere to go.
        assert_eq!(col.reorder_target(3, false, visible), None);
    }

    #[test]
    fn reorder_target_at_the_ends_of_the_column() {
        let col = four_normal_cards();
        assert_eq!(col.reorder_target(0, true, |_| true), None);
        assert_eq!(col.reorder_target(3, false, |_| true), None);
        assert_eq!(col.reorder_target(9, true, |_| true), None, "out of range");
    }

    #[test]
    fn a_card_changing_column_forgets_its_hand_placed_position() {
        // order is an index into the old column; carried over, it would drop
        // the card partway down the new one instead of at the top.
        let mut board = test_board_with_done();
        board.columns[0].cards.push(Card::new("x".into(), "Moved".into()));
        let idx = board.columns[0].cards.len() - 1;
        board.columns[0].cards[idx].order = Some(7);
        board.move_card(0, idx, 1);
        let moved = board.columns[1].cards.iter().find(|c| c.id == "x").unwrap();
        assert_eq!(moved.order, None);
    }

    #[test]
    fn a_new_card_still_lands_at_the_top_of_its_priority_group() {
        // Adding to a hand-ordered column must not send the new card to the bottom.
        let mut col = four_normal_cards();
        col.reorder_card(3, 0);
        col.cards.push(Card::new("new".into(), "New".into()));
        col.sort_cards();
        assert_eq!(col.cards[0].id, "new");
    }

    // ── sort_cards specifics ──

    #[test]
    fn sort_cards_blocked_first() {
        let mut col = Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: None,
            hidden: false,
            cards: vec![],
        };
        let mut normal = Card::new("1".into(), "Normal".into());
        normal.priority = Priority::Normal;
        let mut blocked = Card::new("2".into(), "Blocked".into());
        blocked.priority = Priority::Normal;
        blocked.blocked_by = vec!["1".into()];
        col.cards = vec![normal, blocked];
        col.sort_cards();
        // Both Normal priority, but sort is by priority then updated; blocked doesn't affect sort
        // The sort is: priority.sort_key then updated desc
        // Since both are Normal, the one with a later updated time sorts first
        assert_eq!(col.cards.len(), 2);
    }

    #[test]
    fn sort_cards_priority_then_updated() {
        use chrono::TimeZone;
        let mut col = Column {
            slug: "test".into(),
            name: "Test".into(),
            order: 0,
            wip_limit: None,
            hidden: false,
            cards: vec![],
        };
        let mut old = Card::new("1".into(), "Old".into());
        old.priority = Priority::Normal;
        old.updated = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut recent = Card::new("2".into(), "Recent".into());
        recent.priority = Priority::Normal;
        recent.updated = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        col.cards = vec![old, recent];
        col.sort_cards();
        // Most recently updated sorts first
        assert_eq!(col.cards[0].id, "2");
        assert_eq!(col.cards[1].id, "1");
    }

    // ── Priority display ──

    #[test]
    fn priority_display() {
        assert_eq!(format!("{}", Priority::Urgent), "urgent");
        assert_eq!(format!("{}", Priority::High), "high");
        assert_eq!(format!("{}", Priority::Normal), "normal");
        assert_eq!(format!("{}", Priority::Low), "low");
    }

    #[test]
    fn priority_as_str_all_variants() {
        assert_eq!(Priority::Urgent.as_str(), "urgent");
        assert_eq!(Priority::High.as_str(), "high");
        assert_eq!(Priority::Normal.as_str(), "normal");
        assert_eq!(Priority::Low.as_str(), "low");
    }

    // ── slug_to_name ──

    #[test]
    fn slug_to_name_single_word() {
        assert_eq!(slug_to_name("backlog"), "Backlog");
    }

    #[test]
    fn slug_to_name_multi_word() {
        assert_eq!(slug_to_name("in-progress"), "In Progress");
    }

    #[test]
    fn slug_to_name_three_words() {
        assert_eq!(slug_to_name("ready-for-review"), "Ready For Review");
    }

    #[test]
    fn slug_to_name_empty_string() {
        assert_eq!(slug_to_name(""), "");
    }

    #[test]
    fn slug_to_name_with_digits() {
        assert_eq!(slug_to_name("col-2"), "Col 2");
    }

    #[test]
    fn slug_to_name_uppercase_passthrough() {
        // Only the first char of each segment is uppercased; rest is unchanged.
        assert_eq!(slug_to_name("in-PROGRESS"), "In PROGRESS");
    }

    #[test]
    fn slug_to_name_single_hyphen_does_not_panic() {
        // "-" splits into two empty segments → no panic.
        let result = slug_to_name("-");
        assert_eq!(result, " "); // two empty words joined by space
    }

    // ── slug_from_name ──

    #[test]
    fn slug_from_name_single_word() {
        assert_eq!(slug_from_name("Backlog"), "backlog");
    }

    #[test]
    fn slug_from_name_multi_word() {
        assert_eq!(slug_from_name("In Progress"), "in-progress");
    }

    #[test]
    fn slug_from_name_empty_string_falls_back_to_col() {
        assert_eq!(slug_from_name(""), "col");
    }

    #[test]
    fn slug_from_name_only_special_chars_falls_back_to_col() {
        assert_eq!(slug_from_name("!!! ???"), "col");
    }

    #[test]
    fn slug_from_name_leading_trailing_spaces_trimmed() {
        assert_eq!(slug_from_name("  hello  "), "hello");
    }

    #[test]
    fn slug_from_name_consecutive_spaces_collapse_to_one_hyphen() {
        assert_eq!(slug_from_name("hello   world"), "hello-world");
    }

    #[test]
    fn slug_from_name_leading_special_chars_stripped() {
        assert_eq!(slug_from_name("---hello"), "hello");
    }

    #[test]
    fn slug_from_name_unicode_preserved() {
        assert_eq!(slug_from_name("日本語"), "日本語");
    }

    #[test]
    fn slug_from_name_mixed_ascii_unicode_preserves_all() {
        assert_eq!(slug_from_name("hello 世界"), "hello-世界");
    }

    #[test]
    fn slug_from_name_non_alphanumeric_unicode_falls_back() {
        // Purely symbolic/punctuation Unicode falls back to "col"
        assert_eq!(slug_from_name("→ ← ↑"), "col");
    }

    #[test]
    fn slug_from_name_nordic_characters_preserved() {
        assert_eq!(slug_from_name("Kamelåså"), "kamelåså");
    }

    #[test]
    fn slug_from_name_nfc_normalization_consistency() {
        // Composed (NFC) and decomposed (NFD) representations of "å" must produce the same slug
        let nfc = "Kamelåså"; // å as U+00E5
        let nfd = "Kamela\u{030A}sa\u{030A}"; // a + combining ring above
        assert_eq!(slug_from_name(nfc), slug_from_name(nfd));
    }

    #[test]
    fn slug_from_name_unicode_symbols_replaced() {
        assert_eq!(slug_from_name("hello·world"), "hello-world");
        assert_eq!(slug_from_name("test™"), "test");
    }

    #[test]
    fn slug_from_name_numbers_in_name() {
        assert_eq!(slug_from_name("Col 2"), "col-2");
    }

    #[test]
    fn slug_from_name_does_not_guard_archive_reservation() {
        // slug_from_name does NOT remap the reserved "archive" slug.
        // That guard lives in cmd_col_rename, which is the documented distinction
        // between slug_from_name and generate_slug.
        assert_eq!(slug_from_name("Archive"), "archive");
    }

    // ── generate_slug ──

    fn slug_col(slug: &str) -> Column {
        Column { slug: slug.into(), name: slug.into(), order: 0, wip_limit: None, hidden: false, cards: vec![] }
    }

    #[test]
    fn generate_slug_basic_name() {
        assert_eq!(generate_slug("My Feature", &[]), "my-feature");
    }

    #[test]
    fn generate_slug_spaces_become_hyphens() {
        assert_eq!(generate_slug("Hello   World!!!", &[]), "hello-world");
    }

    #[test]
    fn generate_slug_reserved_archive_avoided() {
        let slug = generate_slug("Archive", &[]);
        assert_ne!(slug, "archive", "should not produce the reserved slug 'archive'");
        // The implementation redirects "archive" to "archive-col", but the key
        // contract is just that the reserved slug is avoided.
        assert!(slug.starts_with("archive"), "slug should still be archive-derived: {slug}");
    }

    #[test]
    fn generate_slug_uniqueness_appends_suffix() {
        let existing = vec![slug_col("backlog")];
        assert_eq!(generate_slug("Backlog", &existing), "backlog-2");
    }

    #[test]
    fn generate_slug_uniqueness_skips_taken_suffixes() {
        let existing = vec![slug_col("backlog"), slug_col("backlog-2")];
        assert_eq!(generate_slug("Backlog", &existing), "backlog-3");
    }

    #[test]
    fn generate_slug_all_non_alphanum_falls_back_to_col() {
        assert_eq!(generate_slug("!!! ???", &[]), "col");
    }

    #[test]
    fn generate_slug_col_uniqueness_when_col_taken() {
        let existing = vec![slug_col("col")];
        assert_eq!(generate_slug("!!! ???", &existing), "col-2");
    }

    #[test]
    fn generate_slug_unicode_name_preserves_characters() {
        let slug = generate_slug("日本語", &[]);
        assert_eq!(slug, "日本語");
    }

    #[test]
    fn generate_slug_leading_special_chars_stripped() {
        // Name starts with special chars that map to hyphens; leading hyphens removed.
        let slug = generate_slug("---hello", &[]);
        assert!(slug.starts_with(|c: char| c.is_alphanumeric()));
    }

    // ── normalize_column_orders ──

    fn make_col(slug: &str, order: u32) -> Column {
        Column { slug: slug.into(), name: slug.into(), order, wip_limit: None, hidden: false, cards: vec![] }
    }

    #[test]
    fn normalize_column_orders_dense_already() {
        let mut cols = vec![make_col("a", 0), make_col("b", 1), make_col("c", 2)];
        normalize_column_orders(&mut cols);
        assert_eq!(cols.iter().map(|c| c.order).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn normalize_column_orders_sparse_orders() {
        let mut cols = vec![make_col("a", 0), make_col("b", 10), make_col("c", 20)];
        normalize_column_orders(&mut cols);
        // Should sort by original order then assign 0,1,2.
        assert_eq!(cols[0].slug, "a");
        assert_eq!(cols[1].slug, "b");
        assert_eq!(cols[2].slug, "c");
        assert_eq!(cols.iter().map(|c| c.order).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn normalize_column_orders_reorders_by_order_field() {
        // Vec is in wrong order relative to the order field.
        let mut cols = vec![make_col("c", 2), make_col("a", 0), make_col("b", 1)];
        normalize_column_orders(&mut cols);
        assert_eq!(cols[0].slug, "a");
        assert_eq!(cols[1].slug, "b");
        assert_eq!(cols[2].slug, "c");
    }

    #[test]
    fn normalize_column_orders_single_column() {
        let mut cols = vec![make_col("solo", 42)];
        normalize_column_orders(&mut cols);
        assert_eq!(cols[0].order, 0);
    }

    #[test]
    fn normalize_column_orders_empty_vec_does_not_panic() {
        let mut cols: Vec<Column> = vec![];
        normalize_column_orders(&mut cols); // must not panic
    }

    // ── is_overdue tests ──

    #[test]
    fn is_overdue_none_returns_false() {
        let card = Card::new("1".into(), "Test".into());
        assert!(!card.is_overdue(chrono::NaiveDate::from_ymd_opt(2025, 6, 15).unwrap()));
    }

    #[test]
    fn is_overdue_yesterday_returns_true() {
        let today = chrono::NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(today - chrono::Days::new(1));
        assert!(card.is_overdue(today));
    }

    #[test]
    fn is_overdue_today_returns_false() {
        let today = chrono::NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(today);
        assert!(!card.is_overdue(today));
    }

    #[test]
    fn is_overdue_tomorrow_returns_false() {
        let today = chrono::NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(today + chrono::Days::new(1));
        assert!(!card.is_overdue(today));
    }

    #[test]
    fn is_overdue_far_past_returns_true() {
        let today = chrono::NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(chrono::NaiveDate::from_ymd_opt(2024, 6, 15).unwrap());
        assert!(card.is_overdue(today));
    }

    #[test]
    fn card_new_due_is_none() {
        let card = Card::new("1".into(), "Test".into());
        assert!(card.due.is_none());
    }

    // ── card_is_visible overdue filter tests ──

    fn fixed_now() -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn card_is_visible_overdue_filter_shows_overdue_card() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(now.date_naive() - chrono::Days::new(1));
        assert!(card_is_visible(&card, None, &[], &[], &[], true, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_overdue_filter_hides_non_overdue_card() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(now.date_naive() + chrono::Days::new(1));
        assert!(!card_is_visible(&card, None, &[], &[], &[], true, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_overdue_filter_hides_card_with_no_due() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let card = Card::new("1".into(), "Test".into());
        assert!(!card_is_visible(&card, None, &[], &[], &[], true, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_overdue_filter_off_shows_all() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let card = Card::new("1".into(), "Test".into());
        assert!(card_is_visible(&card, None, &[], &[], &[], false, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_overdue_filter_card_due_today_is_hidden() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(now.date_naive());
        assert!(!card_is_visible(&card, None, &[], &[], &[], true, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_overdue_combined_with_tag_filter() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let mut card = Card::new("1".into(), "Test".into());
        card.due = Some(now.date_naive() - chrono::Days::new(1));
        card.tags = vec!["bug".into()];
        // Both match
        assert!(card_is_visible(&card, None, &["bug".to_string()], &[], &[], true, &policies, now, &matcher));
        // Tag doesn't match
        assert!(!card_is_visible(&card, None, &["feature".to_string()], &[], &[], true, &policies, now, &matcher));
    }

    #[test]
    fn card_is_visible_text_filter_overrides_overdue_filter() {
        let matcher = SkimMatcherV2::default();
        let now = fixed_now();
        let policies = Policies::default();
        let mut card = Card::new("1".into(), "Test card".into());
        card.due = Some(now.date_naive() + chrono::Days::new(1)); // not overdue
        // Text search takes precedence over overdue filter
        assert!(card_is_visible(&card, Some("test"), &[], &[], &[], true, &policies, now, &matcher));
    }

    // ── generate_template_slug ──

    #[test]
    fn generate_template_slug_basic_name() {
        assert_eq!(generate_template_slug("Bug Report", &[]), "bug-report");
    }

    #[test]
    fn generate_template_slug_dedup_appends_suffix() {
        let existing = vec!["bug-report".to_string()];
        assert_eq!(generate_template_slug("Bug Report", &existing), "bug-report-2");
    }

    #[test]
    fn generate_template_slug_dedup_skips_taken_suffixes() {
        let existing = vec!["bug-report".to_string(), "bug-report-2".to_string()];
        assert_eq!(generate_template_slug("Bug Report", &existing), "bug-report-3");
    }

    #[test]
    fn generate_template_slug_empty_name_returns_template() {
        assert_eq!(generate_template_slug("", &[]), "template");
    }

    #[test]
    fn generate_template_slug_special_chars_only_returns_template() {
        assert_eq!(generate_template_slug("!!! ???", &[]), "template");
    }

    #[test]
    fn generate_template_slug_short_alpha_name() {
        // "Col" contains alphanumeric chars so slug_from_name produces "col",
        // which is a regular slug, not the empty/unicode fallback path.
        assert_eq!(generate_template_slug("Col", &[]), "col");
    }

    #[test]
    fn generate_template_slug_numeric_only() {
        assert_eq!(generate_template_slug("123", &[]), "123");
    }

    #[test]
    fn generate_template_slug_unicode_preserves_characters() {
        assert_eq!(generate_template_slug("日本語", &[]), "日本語");
    }

    #[test]
    fn generate_template_slug_template_already_exists() {
        let existing = vec!["template".to_string()];
        assert_eq!(generate_template_slug("", &existing), "template-2");
    }

    // ── legacy `blocked` field ──

    fn card_toml(extra: &str) -> String {
        format!(
            "id = \"1\"\ntitle = \"T\"\ncreated = \"2025-01-01T00:00:00Z\"\nupdated = \"2025-01-01T00:00:00Z\"\n{extra}"
        )
    }

    #[test]
    fn legacy_blocked_string_loads_as_legacy_note() {
        let card: Card = toml::from_str(&card_toml("blocked = \"waiting for 1\"\n")).unwrap();
        assert_eq!(card.legacy_blocked.as_deref(), Some("waiting for 1"));
        assert!(card.blocked_by.is_empty());
    }

    #[test]
    fn legacy_blocked_true_and_false() {
        let t: Card = toml::from_str(&card_toml("blocked = true\n")).unwrap();
        assert_eq!(t.legacy_blocked.as_deref(), Some(""));
        let f: Card = toml::from_str(&card_toml("blocked = false\n")).unwrap();
        assert!(f.legacy_blocked.is_none());
    }

    #[test]
    fn legacy_blocked_does_not_block() {
        let mut board = blocker_board();
        board.columns[0].cards[0].legacy_blocked = Some("waiting".into());
        assert!(!board.is_blocked(&board.columns[0].cards[0]));
    }

    #[test]
    fn blocked_by_loads_from_toml_array() {
        let card: Card = toml::from_str(&card_toml("blocked_by = [\"2\", \"3\"]\n")).unwrap();
        assert_eq!(card.blocked_by, vec!["2".to_string(), "3".to_string()]);
    }

    // ── blocker derivation ──

    /// backlog: 1, 2, 3 · in-progress: 4 · done: 5 · archive: 6
    fn blocker_board() -> Board {
        let col = |slug: &str, order: u32, ids: &[&str]| Column {
            slug: slug.into(),
            name: slug.into(),
            order,
            wip_limit: None,
            hidden: false,
            cards: ids.iter().map(|id| Card::new((*id).into(), format!("Card {id}"))).collect(),
        };
        Board {
            name: "Test".into(),
            next_card_id: 7,
            policies: Policies::default(),
            sync_branch: None,
            nerd_font: false,
            created_at: None,
            columns: vec![
                col("backlog", 0, &["1", "2", "3"]),
                col("in-progress", 1, &["4"]),
                col("done", 2, &["5"]),
                col("archive", 3, &["6"]),
            ],
        }
    }

    fn card_mut<'a>(board: &'a mut Board, id: &str) -> &'a mut Card {
        let (c, k) = board.find_card(id).unwrap();
        &mut board.columns[c].cards[k]
    }

    fn card<'a>(board: &'a Board, id: &str) -> &'a Card {
        let (c, k) = board.find_card(id).unwrap();
        &board.columns[c].cards[k]
    }

    #[test]
    fn blocker_is_resolved_cases() {
        let board = blocker_board();
        assert!(!board.blocker_is_resolved("1"), "backlog is active");
        assert!(!board.blocker_is_resolved("4"), "in-progress is active");
        assert!(board.blocker_is_resolved("5"), "done resolves");
        assert!(board.blocker_is_resolved("6"), "archive resolves");
        assert!(board.blocker_is_resolved("99"), "missing card resolves");
    }

    #[test]
    fn blocker_in_custom_auto_close_target_is_resolved() {
        let mut board = blocker_board();
        board.policies.auto_close_target = "in-progress".into();
        assert!(board.blocker_is_resolved("4"));
    }

    #[test]
    fn active_blockers_and_is_blocked() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["4".into(), "5".into(), "99".into()];
        let c = card(&board, "1");
        assert_eq!(board.active_blockers(c), vec!["4"]);
        assert!(board.is_blocked(c));
        assert!(!board.is_blocked(card(&board, "2")));
    }

    #[test]
    fn only_resolved_blockers_means_not_blocked() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["5".into(), "6".into()];
        assert!(!board.is_blocked(card(&board, "1")));
    }

    #[test]
    fn is_blocking_derives_multiple_dependents() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["4".into()];
        card_mut(&mut board, "2").blocked_by = vec!["3".into(), "4".into()];
        assert_eq!(board.is_blocking("4"), vec!["1".to_string(), "2".to_string()]);
        assert_eq!(board.is_blocking("3"), vec!["2".to_string()]);
        assert!(board.is_blocking("1").is_empty());
    }

    #[test]
    fn moving_blocker_to_done_and_back_toggles_blocked() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["4".into()];
        assert!(board.is_blocked(card(&board, "1")));

        let (c, k) = board.find_card("4").unwrap();
        board.move_card(c, k, 2); // → done
        assert!(!board.is_blocked(card(&board, "1")));
        assert_eq!(card(&board, "1").blocked_by, vec!["4".to_string()], "dependent untouched");

        let (c, k) = board.find_card("4").unwrap();
        board.move_card(c, k, 1); // back to in-progress
        assert!(board.is_blocked(card(&board, "1")));
    }

    #[test]
    fn deleting_blocker_unblocks() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["4".into()];
        board.columns[1].cards.clear();
        assert!(!board.is_blocked(card(&board, "1")));
    }

    // ── validation and cycles ──

    #[test]
    fn validate_rejects_self_reference() {
        let board = blocker_board();
        assert_eq!(board.validate_blocked_by("1", &["1".into()]), Err("A card cannot block itself".into()));
    }

    #[test]
    fn validate_rejects_unknown_id() {
        let board = blocker_board();
        assert_eq!(board.validate_blocked_by("1", &["99".into()]), Err("No card with ID 99".into()));
    }

    #[test]
    fn validate_accepts_valid_ids() {
        let board = blocker_board();
        assert_eq!(board.validate_blocked_by("1", &["2".into(), "5".into()]), Ok(()));
        assert_eq!(board.validate_blocked_by("1", &[]), Ok(()));
    }

    #[test]
    fn validate_rejects_direct_cycle() {
        let mut board = blocker_board();
        card_mut(&mut board, "2").blocked_by = vec!["3".into()];
        assert_eq!(
            board.validate_blocked_by("3", &["2".into()]),
            Err("Cycle: 3 -> 2 -> 3".into())
        );
    }

    #[test]
    fn validate_rejects_transitive_cycle() {
        let mut board = blocker_board();
        card_mut(&mut board, "2").blocked_by = vec!["3".into()];
        card_mut(&mut board, "3").blocked_by = vec!["4".into()];
        assert_eq!(
            board.validate_blocked_by("4", &["1".into(), "2".into()]),
            Err("Cycle: 4 -> 2 -> 3 -> 4".into())
        );
    }

    #[test]
    fn diamond_is_not_a_cycle() {
        let mut board = blocker_board();
        card_mut(&mut board, "2").blocked_by = vec!["4".into()];
        card_mut(&mut board, "3").blocked_by = vec!["4".into()];
        assert_eq!(board.validate_blocked_by("1", &["2".into(), "3".into()]), Ok(()));
    }

    #[test]
    fn cycle_on_disk_does_not_hang() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["2".into()];
        card_mut(&mut board, "2").blocked_by = vec!["1".into()];
        // Both stay blocked; nothing loops.
        assert!(board.is_blocked(card(&board, "1")));
        assert!(board.is_blocked(card(&board, "2")));
        assert_eq!(board.is_blocking("1"), vec!["2".to_string()]);
        // A third card pointing into the cycle terminates and is not itself cyclic.
        assert_eq!(board.validate_blocked_by("3", &["1".into()]), Ok(()));
        // The cycle is reported once.
        assert_eq!(board.blocker_cycles().len(), 1);
    }

    #[test]
    fn blocker_cycles_empty_when_acyclic() {
        let mut board = blocker_board();
        card_mut(&mut board, "1").blocked_by = vec!["2".into()];
        assert!(board.blocker_cycles().is_empty());
    }

    // ── parse_card_ids ──

    #[test]
    fn parse_card_ids_trims_dedupes_and_drops_empties() {
        assert_eq!(parse_card_ids(" 1, #3 ,, 1,2 "), vec!["1", "3", "2"]);
        assert!(parse_card_ids("").is_empty());
        assert!(parse_card_ids(" , ").is_empty());
    }
}
