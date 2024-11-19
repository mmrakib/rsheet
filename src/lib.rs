mod util;

// rsheet_lib imports
use rsheet_lib::command::{Command, CellIdentifier};
use rsheet_lib::connect::{Connection, Manager, Reader, Writer, ReadMessageResult, WriteMessageResult};
use rsheet_lib::replies::Reply;
use rsheet_lib::cell_expr::{CellExpr, CellArgument};
use rsheet_lib::cell_value::CellValue;

// Standard lib imports
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::collections::HashMap;
use std::error::Error;

// Internal imports
use crate::util::cell_id_to_string;

// Type declarations
type CellGrid = Arc<Mutex< HashMap<String, CellValue> >>;
type LockedCellGrid<'a> = MutexGuard<'a, HashMap<String, CellValue> >;

pub fn start_server<M>(mut manager: M) -> Result<(), Box<dyn Error>>
where
    M: Manager,
{
    // Cell grid
    let cells: CellGrid = Arc::new(Mutex::new(HashMap::new()));

    loop {
        // Initiate client connection
        match manager.accept_new_connection() {
            Connection::NewConnection { reader, writer } => {
                let cells = Arc::clone(&cells);
                thread::spawn(move || handle_client(reader, writer, cells));
            },
            Connection::NoMoreConnections => return Ok((),)
        }
    }
}

fn handle_client(mut reader: impl Reader, mut writer: impl Writer, cells: CellGrid) {
    loop {
        // Read request message from client
        let message: ReadMessageResult = reader.read_message();

        match message {
            ReadMessageResult::Message(msg) => {
                // Handle command and get reply
                let reply = handle_command(msg, &cells);

                // Write reply message to client
                match writer.write_message(reply) {
                    WriteMessageResult::Ok => continue, // Message sent successfully
                    WriteMessageResult::ConnectionClosed => break, // Connection closed, terminate
                    WriteMessageResult::Err(e) => break, // Unexpected error occurred
                }
            },
            ReadMessageResult::ConnectionClosed => break, // Connection closed, terminate
            ReadMessageResult::Err(e) => break, // Unexpected error occurred
        }
    }
}

fn handle_command(command_str: String, cells: &CellGrid) -> Reply {
    let command: Command = match command_str.parse::<Command>() {
        Ok(command) => command,
        Err(e) => return Reply::Error(e.to_string()),
    };

    match command {
        Command::Get { cell_identifier } => {
            let cells = cells.lock().unwrap();
            let cell_id_str = cell_id_to_string(cell_identifier);

            let cell_value = cells.get(&cell_id_str).unwrap_or(&CellValue::None);

            Reply::Value(cell_id_str, cell_value.clone())
        }
        Command::Set { cell_identifier, cell_expr } => {
            let mut cells = cells.lock().unwrap();
            let cell_id_str = cell_id_to_string(cell_identifier);

            let cell_expr = CellExpr::new(&cell_expr);
            let context: HashMap<String, CellArgument> = handle_context(&cell_expr, &cells);
            
            let eval_value = cell_expr.evaluate(&context);

            let cell_value = match eval_value {
                Ok(value) => value,
                Err(_) => return Reply::Error("could not evaluate expression".to_string()),
            };

            cells.insert(cell_id_str.clone(), cell_value.clone());

            Reply::Value(cell_id_str, cell_value.clone())
        }
    }
}

fn handle_context(cell_expr: &CellExpr, cells: &LockedCellGrid) -> HashMap<String, CellArgument> {
    let mut context: HashMap<String, CellArgument> = HashMap::new();
    let variables = cell_expr.find_variable_names();

    for var in variables {
        if let Some(cell_arg) = resolve_variable(&var, cells) {
            context.insert(var.clone(), cell_arg);
        }
    }

    context
}

fn resolve_variable(var_name: &str, cells: &LockedCellGrid) -> Option<CellArgument> {
    if let Some(cell_value) = cells.get(var_name) {
        // Values
        return match cell_value {
            CellValue::Int(i) => Some(CellArgument::Value( CellValue::Int(*i) )),
            CellValue::String(s) => Some(CellArgument::Value( CellValue::String(s.clone()) )),
            CellValue::None => Some(CellArgument::Value( CellValue::Int(0) )),
            CellValue::Error(_) => None,
        };
    }
    
    if let Some(range) = parse_range(var_name) {
        if is_same_row(&range[0], &range[range.len() - 1]) ||
           is_same_col(&range[0], &range[range.len() - 1]) {
            let vector_values = range
                .iter()
                .map(|cell| cells.get(cell).cloned().unwrap_or(CellValue::None))
                .collect::<Vec<_>>();

            return Some(CellArgument::Vector(vector_values));
        } else {
            let matrix = build_matrix(&range, cells);

            return Some(CellArgument::Matrix(matrix));
        }
    }

    None
}

fn parse_range(var_name: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = var_name.split('_').collect();

    if parts.len() != 2 {
        return None;
    }

    let start = parts[0].parse::<CellIdentifier>().ok()?;
    let end = parts[1].parse::<CellIdentifier>().ok()?;

    let mut cells = Vec::new();

    for col in start.col..=end.col {
        for row in start.row..=end.row {
            let cell_id = CellIdentifier { col, row };

            cells.push(cell_id_to_string(cell_id));
        }
    }

    Some(cells)
}

fn is_same_row(start: &str, end: &str) -> bool {
    let start_id = start.parse::<CellIdentifier>().ok();
    let end_id = end.parse::<CellIdentifier>().ok();

    if let (Some(start_id), Some(end_id)) = (start_id, end_id) {
        start_id.row == end_id.row
    } else {
        false
    }
}

fn is_same_col(start: &str, end: &str) -> bool {
    let start_id = start.parse::<CellIdentifier>().ok();
    let end_id = end.parse::<CellIdentifier>().ok();

    if let (Some(start_id), Some(end_id)) = (start_id, end_id) {
        start_id.col == end_id.col
    } else {
        false
    }
}

fn build_matrix(range: &[String], cells: &LockedCellGrid) -> Vec<Vec<CellValue>> {
    let start_id = range.first().and_then(|c| c.parse::<CellIdentifier>().ok()).unwrap();
    let end_id = range.last().and_then(|c| c.parse::<CellIdentifier>().ok()).unwrap();

    let mut matrix = Vec::new();

    for row in start_id.row..=end_id.row {
        let mut row_values = Vec::new();
        for col in start_id.col..=end_id.col {
            let cell_id = CellIdentifier { col, row };
            let cell_name = cell_id_to_string(cell_id);
            let value = cells.get(&cell_name).cloned().unwrap_or(CellValue::None);
            row_values.push(value);
        }
        matrix.push(row_values);
    }

    matrix
}
