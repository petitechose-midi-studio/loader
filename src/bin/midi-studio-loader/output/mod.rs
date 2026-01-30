use crate::cli;

use midi_studio_loader::{api, targets};

pub mod human;
pub mod json;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy)]
pub struct OutputOptions {
    pub json: bool,
    pub verbose: bool,
    pub quiet: bool,
}

pub trait Output {
    fn options(&self) -> OutputOptions;

    fn json_line(&mut self, value: serde_json::Value);
    fn json_event(&mut self, ev: json::JsonEvent);
    fn human_line(&mut self, msg: &str);
    fn error(&mut self, code: i32, msg: &str);

    fn flash_event(&mut self, ev: api::FlashEvent);
    fn ambiguous_help(&mut self);
    fn finish(&mut self);
}

pub fn make_for_flash(args: &cli::FlashArgs) -> Box<dyn Output> {
    let opts = OutputOptions {
        json: args.json,
        verbose: args.verbose,
        quiet: args.quiet,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts).with_wait(args.wait))
    }
}

pub fn make_for_reboot(args: &cli::RebootArgs) -> Box<dyn Output> {
    let opts = OutputOptions {
        json: args.json,
        verbose: args.verbose,
        quiet: false,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts))
    }
}

pub fn make_for_list(args: &cli::ListArgs) -> Box<dyn Output> {
    let opts = OutputOptions {
        json: args.json,
        verbose: false,
        quiet: false,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts))
    }
}

pub fn make_for_doctor(args: &cli::DoctorArgs) -> Box<dyn Output> {
    let opts = OutputOptions {
        json: args.json,
        verbose: false,
        quiet: false,
    };
    if args.json {
        Box::new(json::JsonOutput::new(opts))
    } else {
        Box::new(human::HumanOutput::new(opts))
    }
}

pub fn target_to_value(index: usize, t: &targets::Target) -> serde_json::Value {
    let mut v = serde_json::to_value(t)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(obj) = &mut v {
        obj.insert("index".to_string(), serde_json::Value::from(index as u64));
        obj.insert("id".to_string(), serde_json::Value::from(t.id()));
    }
    v
}
