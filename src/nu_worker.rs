use std::env;
use std::process;

// This function is called BEFORE the GUI initializes if the --nu-worker flag is detected.
pub fn run() {
    let args: Vec<String> = env::args().collect();
    
    // Expected format: grm --nu-worker <script.nu> <json_args>
    if args.len() < 3 {
        eprintln!("Error: Nu worker requires a script path.");
        process::exit(1);
    }

    let script_path = &args[2];
    
    // In a full implementation, you would parse the optional json_args (args[3]) 
    // and inject them into the Nu environment variables here.

    // 1. Initialize the Nushell Engine State
    let mut engine_state = nu_command::add_shell_command_context(nu_cmd_lang::create_default_context());
    engine_state = nu_cli::add_cli_context(engine_state);
    
    // Manually add nu_cli::Print since add_cli_context doesn't do it
    let mut working_set = nu_protocol::engine::StateWorkingSet::new(&engine_state);
    working_set.add_decl(Box::new(nu_cli::Print));
    let delta = working_set.render();
    engine_state.merge_delta(delta).expect("Failed to add Print");
    
    let mut stack = nu_protocol::engine::Stack::new();
    
    // Nushell requires PWD and other env vars to be set in the environment
    for (k, v) in std::env::vars() {
        stack.add_env_var(
            k,
            nu_protocol::Value::string(v, nu_protocol::Span::unknown()),
        );
    }
    if let Ok(cwd) = std::env::current_dir() {
        stack.add_env_var(
            "PWD".into(),
            nu_protocol::Value::string(cwd.to_string_lossy().to_string(), nu_protocol::Span::unknown()),
        );
    }

    // 2. Load the standard library (optional but recommended for basic scripting)
    // nu_std::load_standard_library(&mut engine_state).unwrap();

    // 3. Read the script
    let mut script_content = match std::fs::read_to_string(script_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Worker Error: Failed to read script '{}': {}", script_path, e);
            process::exit(1);
        }
    };

    // If GRM_ASSET_PATH is present, it means we are invoked from GRM pipeline runner.
    // Append a call to main at the end of the script so eval_block executes it.
    if std::env::var("GRM_ASSET_PATH").is_ok() {
        script_content.push_str("\n\nmain $env.GRM_ASSET_PATH $env.GRM_INSTALL_DIR\n");
    }

    // 4. Parse the script
    let mut working_set = nu_protocol::engine::StateWorkingSet::new(&engine_state);
    let block = nu_parser::parse(&mut working_set, Some(script_path), script_content.as_bytes(), false);
    
    if !working_set.parse_errors.is_empty() {
        for err in working_set.parse_errors {
            eprintln!("Parse Error: {:?}", err);
        }
        process::exit(1);
    }

    let delta = working_set.render();
    if let Err(err) = engine_state.merge_delta(delta) {
        eprintln!("Engine Error: {:?}", err);
        process::exit(1);
    }

    // 5. Evaluate the script
    match nu_engine::eval_block::<nu_protocol::debugger::WithoutDebug>(&engine_state, &mut stack, &block, nu_protocol::PipelineData::empty()) {
        Ok(pipeline_data) => {
            // Drain the output to stdout so the GUI parent process can read it
            for item in pipeline_data.body.into_iter() {
                match item.as_str() {
                    Ok(s) => println!("{}", s),
                    Err(_) => println!("{:?}", item),
                }
            }
            process::exit(0);
        }
        Err(err) => {
            eprintln!("Execution Error: {:?}", err);
            process::exit(1);
        }
    }
}
