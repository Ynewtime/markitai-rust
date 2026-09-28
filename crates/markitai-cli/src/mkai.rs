mod app;
mod output_claims;
mod report;
mod report_store;
mod run_state;
mod signals;
fn main() {
    std::process::exit(app::run());
}
