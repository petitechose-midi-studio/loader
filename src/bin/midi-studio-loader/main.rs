use std::process;

use clap::Parser;

mod cli;
mod commands;
mod context;
mod exit_codes;
mod logging;
mod output;

fn main() {
    logging::init_tracing();

    let cli = cli::Cli::parse();

    let exit_code = match cli.command {
        cli::Command::Flash(args) => {
            let mut out = output::make_for_flash(&args);
            let code = commands::flash::run(args, &mut *out);
            out.finish();
            code
        }
        cli::Command::Reboot(args) => {
            let mut out = output::make_for_reboot(&args);
            let code = commands::reboot::run(args, &mut *out);
            out.finish();
            code
        }
        cli::Command::List(args) => {
            let mut out = output::make_for_list(&args);
            let code = commands::list::run(args, &mut *out);
            out.finish();
            code
        }
        cli::Command::Doctor(args) => {
            let mut out = output::make_for_doctor(&args);
            let code = commands::doctor::run(args, &mut *out);
            out.finish();
            code
        }
    };

    process::exit(exit_code);
}
