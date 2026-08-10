//! Finding a piece of text across a project.
//!
//! The tree answers "where is the file called this"; this answers "where is the
//! thing I wrote", which in a project of a dozen `.swync` files is the question
//! that actually gets asked — where a `fn` is defined, everywhere a sample is
//! loaded, which piece still uses the old name for something.
//!
//! Plain substrings, not patterns. A regular expression is a second language to
//! get wrong in a box that is meant to be typed into without thinking, and
//! nothing in swync's own syntax is hard to search for literally. Case and whole
//! words are the two distinctions that come up in practice, and both are
//! toggles.
//!
//! Every limit here exists because a project is a folder somebody chose, and
//! nothing stops that folder being enormous. A search that would go on forever
//! stops and says it stopped, which is more use than one that never answers.

use std::path::Path;

use super::walk;

/// Files bigger than this are not searched.
///
/// Text somebody typed is never this large. What is, is a sample, a render, or
/// a database, and reading one into memory to look for `kick` in it helps
/// nobody.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// How many matches are collected before the search gives up and says so.
const MAX_MATCHES: usize = 1_000;

/// How much of a line the panel is shown, when the line is too long to draw.
const MAX_SNIPPET: usize = 240;

/// How much of a long line to keep ahead of the match, so it has some context
/// on the left rather than starting exactly at the hit.
const SNIPPET_LEAD: usize = 40;

/// One occurrence, as the panel draws it.
///
/// The snippet arrives in three pieces rather than as a line and a pair of
/// offsets. Offsets would have to survive Rust counting characters and
/// JavaScript counting UTF-16 units, which are not the same number the moment
/// anybody writes an emoji in a comment — and the panel does not want the
/// positions anyway, it wants to put a mark around the middle piece.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Match {
    /// 1-based, so it can be handed to the editor's own jump.
    pub line: usize,
    /// 1-based, counted in characters, for the same reason. Always into the
    /// real line, never into the snippet, which may be a window cut out of it.
    pub column: usize,
    /// What is on the line before the match: indentation dropped, and cut down
    /// with an ellipsis if the line was long.
    pub before: String,
    /// The matched text, exactly as the file writes it — which is not
    /// necessarily how it was typed into the box, since a search may ignore
    /// case.
    pub hit: String,
    /// The rest of the line, cut down the same way.
    pub after: String,
}

/// One file's occurrences.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct FileMatches {
    pub path: String,
    /// The file's own name, so the panel's heading needs no path parsing.
    pub name: String,
    pub matches: Vec<Match>,
}

/// What a search found.
#[derive(serde::Serialize, Debug, PartialEq, Default)]
pub struct Results {
    pub files: Vec<FileMatches>,
    /// Across all files, so the panel can say how many there were without
    /// adding up a list it may only be showing part of.
    pub total: usize,
    /// True when the search stopped at a limit rather than at the end of the
    /// project, so the panel can say the list is not all of them.
    pub truncated: bool,
}

/// Search a project's files for a piece of text.
///
/// An empty query finds nothing rather than everything: the box is empty before
/// anyone has typed in it, and that is not a request for every line in the
/// project.
#[tauri::command]
pub fn search_project(
    root: String,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<Results, String> {
    if query.is_empty() {
        return Ok(Results::default());
    }

    let root = Path::new(&root);
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }

    let mut paths = walk(root);
    // So the panel's list reads in the order the tree does, and so that two
    // searches of an unchanged project agree with each other.
    paths.sort();

    let mut results = Results::default();

    for path in paths {
        if results.total >= MAX_MATCHES {
            results.truncated = true;
            break;
        }
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            continue;
        }
        // Anything that is not text fails here, which is the right answer for a
        // sample or an image: there is nothing in one to find.
        let Ok(code) = std::fs::read_to_string(&path) else { continue };

        let budget = MAX_MATCHES - results.total;
        let matches = in_file(&code, &query, case_sensitive, whole_word, budget);
        if matches.is_empty() {
            continue;
        }

        results.total += matches.len();
        results.files.push(FileMatches {
            path: path.display().to_string(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            matches,
        });
    }

    if results.total >= MAX_MATCHES {
        results.truncated = true;
    }

    Ok(results)
}

/// Every occurrence in one file's text, up to `budget` of them.
///
/// `needle` is the query as it was typed. Folding it to match the lines is done
/// here rather than by the caller, so there is no way to call this with a
/// half-prepared needle and quietly find nothing.
fn in_file(
    code: &str,
    needle: &str,
    case_sensitive: bool,
    whole_word: bool,
    budget: usize,
) -> Vec<Match> {
    let wanted: Vec<char> = if case_sensitive {
        needle.chars().collect()
    } else {
        needle.to_lowercase().chars().collect()
    };
    let mut found = Vec::new();

    for (number, line) in code.lines().enumerate() {
        if found.len() >= budget {
            break;
        }
        // Searched in a copy folded to the same case as the needle, and then
        // reported against the line as written. Both are walked in characters
        // from here on: lowercasing can change how many bytes a character
        // takes, and it never changes how many characters there are.
        let folded: Vec<char> = if case_sensitive {
            line.chars().collect()
        } else {
            line.to_lowercase().chars().collect()
        };
        let written: Vec<char> = line.chars().collect();
        if folded.len() != written.len() {
            // A character whose lowercase is more than one character — `İ`, say.
            // Rare, and the positions would all be a little wrong, so this line
            // is searched as it stands rather than reported wrongly.
            continue;
        }
        let mut at = 0;
        while let Some(start) = find_from(&folded, &wanted, at) {
            let end = start + wanted.len();
            at = start + 1;

            if whole_word && !is_whole_word(&folded, start, end) {
                continue;
            }

            let (before, hit, after) = snippet(&written, start, end);
            found.push(Match { line: number + 1, column: start + 1, before, hit, after });
            if found.len() >= budget {
                break;
            }
        }
    }

    found
}

/// The first position at or after `from` where `needle` sits in `haystack`.
fn find_from(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// Whether a match is bounded on both sides by something that is not part of a
/// word.
fn is_whole_word(line: &[char], start: usize, end: usize) -> bool {
    let before = start.checked_sub(1).map(|i| line[i]);
    let after = line.get(end).copied();
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The line as the panel should show it: what is before the match, the match,
/// and what is after.
///
/// Indentation goes, because a deeply nested match otherwise arrives as a
/// snippet of mostly spaces. A line too long to read is cut to a window around
/// the match, with an ellipsis on whichever side was cut — the alternative is a
/// results panel that one minified file makes useless.
fn snippet(line: &[char], start: usize, end: usize) -> (String, String, String) {
    let indent = line
        .iter()
        .take_while(|c| c.is_whitespace())
        .count()
        // Never past the match, for a match that is inside the indentation.
        .min(start);

    let hit: String = line[start..end].iter().collect();
    let long = line.len() - indent > MAX_SNIPPET;

    let from = if long { start.saturating_sub(SNIPPET_LEAD).max(indent) } else { indent };
    let mut before: String = line[from..start].iter().collect();
    if from > indent {
        before.insert(0, '…');
    }

    let room = MAX_SNIPPET
        .saturating_sub(before.chars().count() + hit.chars().count())
        .max(1);
    let to = if long { (end + room).min(line.len()) } else { line.len() };
    let mut after: String = line[end..to].iter().collect::<String>().trim_end().to_string();
    if to < line.len() {
        after.push('…');
    }

    (before, hit, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(code: &str, needle: &str) -> Vec<Match> {
        in_file(code, needle, true, false, MAX_MATCHES)
    }

    #[test]
    fn a_match_carries_the_place_the_editor_can_jump_to() {
        let found = find("let kick = 1\nplay(kick)\n", "kick");
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].line, found[0].column), (1, 5));
        assert_eq!((found[1].line, found[1].column), (2, 6));
    }

    /// The three pieces are the line, so nothing is dropped between them.
    #[test]
    fn the_pieces_put_the_line_back_together() {
        let found = find("play(kick, 1)\n", "kick");
        let m = &found[0];
        assert_eq!(format!("{}{}{}", m.before, m.hit, m.after), "play(kick, 1)");
    }

    #[test]
    fn two_matches_on_one_line_are_both_found() {
        let found = find("kick kick\n", "kick");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].column, 1);
        assert_eq!(found[1].column, 6);
    }

    /// Overlapping matches advance rather than looping forever.
    #[test]
    fn a_repeating_needle_terminates() {
        let found = find("aaaa\n", "aa");
        assert_eq!(found.len(), 3, "got {found:?}");
    }

    #[test]
    fn case_can_be_ignored_or_not() {
        assert_eq!(in_file("Kick\n", "kick", true, false, 10).len(), 0);
        assert_eq!(in_file("Kick\n", "kick", false, false, 10).len(), 1);
    }

    /// A case-insensitive match reports the text as the file writes it, and a
    /// position into the real line rather than into the folded copy.
    #[test]
    fn an_ignored_case_match_still_describes_the_real_line() {
        let found = in_file("let KICK = 1\n", "kick", false, false, 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].column, 5);
        assert_eq!(found[0].hit, "KICK");
    }

    #[test]
    fn whole_words_skip_the_ones_inside_other_names() {
        let found = in_file("kick kickdrum sidekick kick_2\n", "kick", true, true, 10);
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].column, 1);
    }

    /// Indentation is dropped from the snippet, and the column is not: one is
    /// what the panel draws, the other is where the editor jumps.
    #[test]
    fn a_snippet_loses_its_indentation() {
        let found = find("        let kick = 1\n", "kick");
        assert_eq!(found[0].before, "let ");
        assert_eq!(found[0].column, 13);
    }

    /// A line too long to read is shown as a window around the match.
    #[test]
    fn a_very_long_line_is_windowed_around_the_match() {
        let line = format!("{}kick{}\n", "x".repeat(500), "y".repeat(500));
        let found = find(&line, "kick");
        assert_eq!(found.len(), 1);
        let m = &found[0];
        let shown = m.before.chars().count() + m.hit.chars().count() + m.after.chars().count();
        assert!(shown <= MAX_SNIPPET + 2, "showed {shown} characters");
        assert!(m.before.starts_with('…') && m.after.ends_with('…'));
        assert_eq!(m.hit, "kick");
        // And the column still describes the real line rather than the window.
        assert_eq!(m.column, 501);
    }

    /// A short line is shown whole, however far along it the match is.
    #[test]
    fn a_line_that_fits_is_not_windowed() {
        let line = format!("{}kick\n", "x".repeat(100));
        let m = &find(&line, "kick")[0];
        assert!(!m.before.starts_with('…'), "got {}", m.before);
        assert_eq!(m.before.chars().count(), 100);
    }

    #[test]
    fn a_match_beyond_the_budget_is_left_out() {
        let found = in_file("kick\nkick\nkick\n", "kick", true, false, 2);
        assert_eq!(found.len(), 2);
    }

    /// Characters rather than bytes throughout: non-ASCII ahead of a match
    /// must not shift where the match is reported to be.
    #[test]
    fn characters_rather_than_bytes_are_counted() {
        let found = in_file("// ölünç kick\n", "KICK", false, false, 10);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hit, "kick");
        assert_eq!(found[0].column, 10);
        assert_eq!(found[0].before, "// ölünç ");
    }
}
