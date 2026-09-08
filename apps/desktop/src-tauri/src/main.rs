#[derive(Debug, PartialEq)]
enum LaunchMode {
    Desktop,
    Mcp,
    NativeUiTest,
    NativePdfTest,
    NativeCompactTest { pdf: bool },
    BuildInfo,
    Help,
}

fn parse_mode(arguments: &[String]) -> Result<LaunchMode, &'static str> {
    // Older Finder versions may provide their process serial number.
    let arguments = arguments
        .iter()
        .filter(|argument| !argument.starts_with("-psn_"))
        .map(String::as_str)
        .collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(LaunchMode::Desktop),
        ["--mcp-stdio"] => Ok(LaunchMode::Mcp),
        ["--build-info"] => Ok(LaunchMode::BuildInfo),
        ["--native-ui-test"] => Ok(LaunchMode::NativeUiTest),
        ["--native-ui-test", "--scenario=pdf"] => Ok(LaunchMode::NativePdfTest),
        ["--native-ui-test", "--compact"] => Ok(LaunchMode::NativeCompactTest { pdf: false }),
        ["--native-ui-test", "--scenario=pdf", "--compact"] => {
            Ok(LaunchMode::NativeCompactTest { pdf: true })
        }
        ["--help"] | ["-h"] => Ok(LaunchMode::Help),
        _ => Err("Unknown or conflicting arguments; no desktop services were started. Use --help."),
    }
}

fn main() {
    let mode = match parse_mode(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let result = match mode {
        LaunchMode::Desktop => {
            pasters_desktop_lib::run();
            Ok(())
        }
        LaunchMode::Mcp => pasters_desktop_lib::mcp::run_stdio().map_err(|error| error.to_string()),
        LaunchMode::NativeUiTest => pasters_desktop_lib::run_native_ui_test(),
        LaunchMode::NativePdfTest => pasters_desktop_lib::run_native_pdf_test(),
        LaunchMode::NativeCompactTest { pdf } => pasters_desktop_lib::run_native_compact_test(pdf),
        LaunchMode::BuildInfo => {
            pasters_desktop_lib::build_info_json().map(|json| println!("{json}"))
        }
        LaunchMode::Help => {
            println!(
                "CopyRail [--mcp-stdio | --native-ui-test [--scenario=pdf] [--compact] | --build-info | --help]\n--native-ui-test: debug-only, fresh synthetic history; no capture, clipboard write commands, cloud or account actions.\n--scenario=pdf: synthetic three-page, locked and invalid documents; requires --native-ui-test.\n--compact: start the isolated fixture in Compact layout; no system preference changes.\n--build-info: public compiled configuration only; no desktop services, clipboard, database or windows."
            );
            Ok(())
        }
    };
    if let Err(error) = result {
        eprintln!("CopyRail failed to start: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn a_misspelled_or_combined_isolation_flag_never_falls_back_to_real_history() {
        for values in [
            vec!["--native-ui-test=path"],
            vec!["--native-ui-tes"],
            vec!["--mcp-stdio", "--native-ui-test"],
            vec!["--native-ui-test", "anything"],
            vec!["--scenario=pdf"],
            vec!["--native-ui-test", "--scenario=private"],
            vec!["--native-ui-test", "--scenario=pdf", "a-user-file.pdf"],
            vec!["--compact"],
            vec!["--native-ui-test", "--compact=true"],
            vec!["--native-ui-test", "--compact", "--compact"],
            vec!["--mcp-stdio", "--compact"],
        ] {
            assert!(parse_mode(&args(&values)).is_err());
        }
    }

    #[test]
    fn startup_modes_are_explicit_and_preserve_finder_launches() {
        assert_eq!(parse_mode(&args(&[])), Ok(LaunchMode::Desktop));
        assert_eq!(parse_mode(&args(&["-psn_0_123"])), Ok(LaunchMode::Desktop));
        assert_eq!(
            parse_mode(&args(&["--native-ui-test"])),
            Ok(LaunchMode::NativeUiTest)
        );
        assert_eq!(parse_mode(&args(&["--mcp-stdio"])), Ok(LaunchMode::Mcp));
        assert_eq!(
            parse_mode(&args(&["--native-ui-test", "--scenario=pdf"])),
            Ok(LaunchMode::NativePdfTest)
        );
        assert_eq!(parse_mode(&args(&["--help"])), Ok(LaunchMode::Help));
    }

    #[test]
    fn compact_layout_requires_an_explicit_isolated_scenario() {
        assert_eq!(
            parse_mode(&args(&["--native-ui-test", "--compact"])),
            Ok(LaunchMode::NativeCompactTest { pdf: false })
        );
        assert_eq!(
            parse_mode(&args(&["--native-ui-test", "--scenario=pdf", "--compact"])),
            Ok(LaunchMode::NativeCompactTest { pdf: true })
        );
        assert!(parse_mode(&args(&["--scenario=pdf", "--compact"])).is_err());
        assert!(
            parse_mode(&args(&[
                "--native-ui-test",
                "--compact",
                "--scenario=unknown"
            ]))
            .is_err()
        );
    }

    #[test]
    fn build_info_is_standalone_and_cannot_fall_through_to_a_desktop_launch() {
        assert_eq!(
            parse_mode(&args(&["--build-info"])),
            Ok(LaunchMode::BuildInfo)
        );
        for values in [
            vec!["--build-inf"],
            vec!["--build-info", "--native-ui-test"],
            vec!["--build-info", "--mcp-stdio"],
            vec!["--build-info", "--compact"],
            vec!["--build-info", "--build-info"],
            vec!["--build-info=/tmp/test"],
        ] {
            assert!(parse_mode(&args(&values)).is_err());
        }
    }
}
