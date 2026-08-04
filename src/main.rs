use std::process::ExitCode;

const CLI_WORKER_STACK_SIZE: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let json = args.iter().any(|argument| argument == "--json");
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(CLI_WORKER_STACK_SIZE)
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return startup_error(json, format!("could not start async runtime: {error}"))
        }
    };

    // Windows gives the process entry thread a smaller stack than Unix. Poll
    // the schema-v3 command graph on an explicitly sized Tokio worker while
    // the entry thread waits only on the small join handle.
    let command = runtime.spawn(a3s_use::cli::run(args));
    match runtime.block_on(command) {
        Ok(result) => render_result(json, result),
        Err(error) => startup_error(json, format!("async command worker failed: {error}")),
    }
}

fn render_result(
    json: bool,
    result: a3s_use_core::UseResult<a3s_use::cli::CommandOutput>,
) -> ExitCode {
    match result {
        Ok(output) => {
            if output.should_print && json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output.json).unwrap_or_default()
                );
            } else if output.should_print && !output.human.is_empty() {
                println!("{}", output.human);
            }
            ExitCode::from(output.exit_code)
        }
        Err(error) => {
            if json {
                let output = serde_json::json!({
                    "schemaVersion": 1,
                    "ok": false,
                    "error": error,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output).unwrap_or_default()
                );
            } else {
                eprintln!("a3s-use: {error}");
                if let Some(suggestion) = &error.suggestion {
                    eprintln!("suggestion: {suggestion}");
                }
            }
            ExitCode::from(1)
        }
    }
}

fn startup_error(json: bool, message: String) -> ExitCode {
    if json {
        let output = serde_json::json!({
            "schemaVersion": 1,
            "ok": false,
            "error": {
                "code": "use.runtime_start_failed",
                "message": message,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_default()
        );
    } else {
        eprintln!("a3s-use: {message}");
    }
    ExitCode::from(1)
}
