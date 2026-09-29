//! Usage hints for removed spellings. Literal option values remain the parser's concern.
use clap::CommandFactory;
use std::ffi::OsString;

const REMOVED: [(&str, Option<&str>); 6] = [
    ("playwright", Some("-s playwright")),
    ("defuddle", Some("-s defuddle")),
    ("static", Some("-s static")),
    ("jina", Some("-s jina")),
    ("cloudflare", Some("-s cloudflare")),
    ("kreuzberg", None),
];

pub(super) fn removed_option(arguments: &[OsString]) -> Option<String> {
    // Ordinary conversions do not build a second Clap command graph.
    if !arguments
        .iter()
        .filter_map(|value| value.to_str())
        .any(|text| {
            text.strip_prefix("--").is_some_and(|long| {
                let name = long.split_once('=').map_or(long, |(name, _)| name);
                REMOVED.iter().any(|(old, _)| *old == name)
            })
        })
    {
        return None;
    }
    let mut root = super::Cli::command();
    root.build();
    let mut command = &root;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        index += 1;
        let Some(text) = argument.to_str() else {
            continue;
        };
        if text == "--" {
            break;
        }
        // auth deliberately accepts an opaque trailing argument vector.
        if command.get_name() == "auth" {
            break;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, attached) = long
                .split_once('=')
                .map_or((long, false), |(name, _)| (name, true));
            if let Some((_, replacement)) = REMOVED.iter().find(|(old, _)| *old == name) {
                return Some(match replacement {
                    Some(replacement) => {
                        format!("--{name} has been removed, use '{replacement}' instead.")
                    }
                    None => format!("--{name} has been removed, RTF converts natively now."),
                });
            }
            if !attached
                && command
                    .get_arguments()
                    .any(|arg| arg.get_long() == Some(name) && arg.get_action().takes_values())
            {
                index += 1;
            }
        } else if let Some(shorts) = text.strip_prefix('-').filter(|text| !text.is_empty()) {
            // A value consumes the rest of a short-option cluster, or the next token.
            for (offset, short) in shorts.char_indices() {
                if command
                    .get_arguments()
                    .any(|arg| arg.get_short() == Some(short) && arg.get_action().takes_values())
                {
                    if offset + short.len_utf8() == shorts.len() {
                        index += 1;
                    }
                    break;
                }
            }
        } else if let Some(child) = command.find_subcommand(text) {
            command = child;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hint(args: &[&str]) -> Option<String> {
        removed_option(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn removed_options_are_recognized_in_conversion_positions() {
        for (name, replacement) in REMOVED {
            for args in [
                vec![format!("--{name}"), "note.txt".into()],
                vec!["note.txt".into(), format!("--{name}=true")],
            ] {
                let words = args.iter().map(String::as_str).collect::<Vec<_>>();
                let message = hint(&words).unwrap();
                assert!(message.contains(&format!("--{name} has been removed")));
                assert!(message.contains(replacement.unwrap_or("RTF converts natively now")));
            }
        }
    }

    #[test]
    fn literal_values_attached_values_and_terminator_are_not_removed_flags() {
        for args in [
            vec!["--", "--static"],
            vec!["-o", "--static", "note.txt"],
            vec!["--output", "--jina", "note.txt"],
            vec!["--output=--cloudflare", "note.txt"],
            vec!["-vo", "--defuddle", "note.txt"],
            vec!["-o--static", "note.txt"],
            vec!["--config-json", "--kreuzberg", "note.txt"],
            vec!["config", "list", "--format", "--static"],
            vec!["auth", "--jina"],
        ] {
            assert_eq!(hint(&args), None, "{args:?}");
        }
        assert!(hint(&["-o", "literal", "note.txt", "--jina"]).is_some());
    }
}
