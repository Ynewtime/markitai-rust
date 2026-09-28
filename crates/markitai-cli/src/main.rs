mod app;
mod report;
mod report_store;
// Storage is validated independently before batch scheduling uses it.
#[allow(dead_code, unused_imports)]
mod run_state;
fn main() {
    std::process::exit(app::run());
}
