mod app;
mod diagnostics;
mod history;
mod mcp;
mod output_claims;
mod pricing;
mod report;
mod report_store;
mod run_state;
mod server;
mod signals;
fn main() {
    std::process::exit(app::run());
}
