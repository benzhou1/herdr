use crate::api::schema::{EmptyParams, Method, Request, ResponseResult, SuccessResponse};

use super::projection::ParseError;
use super::workspace_bindings::{command, move_method, parse_move, LEAVES};

pub(super) fn run_workspace_command(args: &[String]) -> std::io::Result<i32> {
    let print_commands = || eprintln!("{}", super::projection::group_commands(&command()));
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_commands();
        return Ok(2);
    };
    if matches!(subcommand, "help" | "--help" | "-h") {
        print_commands();
        return Ok(0);
    }
    if subcommand == "move" {
        return run_move(&args[1..]);
    }
    let Some(leaf) = LEAVES.iter().find(|leaf| leaf.name == subcommand) else {
        print_commands();
        return Ok(2);
    };
    let request = match leaf.request(&args[1..]) {
        Ok(request) => request,
        Err(error) => return report_parse_error(error),
    };
    if leaf.quiet {
        super::send_ok_request(request.method)
    } else {
        super::print_response(&super::send_request(&request)?)
    }
}

fn report_parse_error(error: ParseError) -> std::io::Result<i32> {
    match error {
        ParseError::Usage(message) => {
            eprintln!("{message}");
            Ok(2)
        }
        ParseError::Io(error) => Err(error),
        ParseError::Clap(error) => super::print_clap_error(&error),
    }
}

/// Reads the current order, then sends one atomic `workspace.move_block`. The
/// order can change between the two calls; the server still validates the ids.
fn run_move(args: &[String]) -> std::io::Result<i32> {
    let move_args = match parse_move(args) {
        Ok(move_args) => move_args,
        Err(error) => return report_parse_error(error),
    };
    let id = "cli:workspace:move";
    let list = super::send_request(&Request {
        id: id.into(),
        method: Method::WorkspaceList(EmptyParams {}),
    })?;
    let Ok(SuccessResponse {
        result: ResponseResult::WorkspaceList { workspaces },
        ..
    }) = serde_json::from_value(list.clone())
    else {
        return super::print_response(&list);
    };
    let order: Vec<String> = workspaces
        .into_iter()
        .map(|workspace| workspace.workspace_id)
        .collect();
    let method = match move_method(&order, move_args) {
        Ok(method) => method,
        Err(error) => return report_parse_error(error),
    };
    super::print_response(&super::send_request(&Request {
        id: id.into(),
        method,
    })?)
}
