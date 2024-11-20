mod util;

// rsheet_lib imports
use rsheet_lib::command::{Command, CellIdentifier};
use rsheet_lib::connect::{Connection, Manager, Reader, Writer, ReadMessageResult, WriteMessageResult};
use rsheet_lib::replies::Reply;
use rsheet_lib::cell_expr::{CellExpr, CellArgument};
use rsheet_lib::cell_value::CellValue;

// Standard lib imports
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{self, Write};

// Internal imports
use crate::util::cell_id_to_string;

// Cells
#[derive(Debug, Clone)]
struct TimedCellValue {
    value: CellValue,
    expression: Option<String>,
    timestamp: Instant,
}
type CellGrid = HashMap<String, TimedCellValue>;

// Dependencies
type DependencyGraph = HashMap<String, Vec<String>>;

// Spreadsheet
#[derive(Debug, Clone)]
struct Spreadsheet {
    cells: CellGrid,
    dependencies: DependencyGraph,
}
type SharedSpreadsheet = Arc<Mutex< Spreadsheet >>;

// Thread handles
type ThreadHandles = Arc<Mutex< Vec<JoinHandle<()> >>>;

pub fn start_server<M>(mut manager: M) -> Result<(), Box<dyn Error>>
where
    M: Manager,
{
    let spreadsheet = Arc::new(Mutex::new(Spreadsheet {
        cells: HashMap::new(),
        dependencies: HashMap::new(),
    }));

    let thread_handles = Arc::new(Mutex::new(Vec::new()));

    loop {
        // Initiate client connection
        match manager.accept_new_connection() {
            Connection::NewConnection { reader, writer } => {
                let spreadsheet = Arc::clone(&spreadsheet);
                let thread_handles_clone = Arc::clone(&thread_handles);

                let handle = thread::spawn(move || handle_client(reader, writer, spreadsheet, thread_handles_clone));
                thread_handles.lock().unwrap().push(handle);
            },
            Connection::NoMoreConnections => {
                wait_for_threads(thread_handles);
                return Ok(());
            },
        }
    }
}

fn handle_client(mut reader: impl Reader, mut writer: impl Writer, spreadsheet: SharedSpreadsheet, thread_handles: ThreadHandles) {
    loop {
        // Read request message from client
        let message: ReadMessageResult = reader.read_message();

        match message {
            ReadMessageResult::Message(msg) => {
                // Handle command and get reply
                let reply = handle_command(msg, &spreadsheet, thread_handles.clone());

                // Write reply message to client
                match writer.write_message(reply) {
                    WriteMessageResult::Ok => { // Message sent successfully
                        if let Err(e) = io::stdout().flush() {
                            eprintln!("error flushing stdout: {}", e);
                        }
                        continue;
                    } 
                    WriteMessageResult::ConnectionClosed => break, // Connection closed, terminate
                    WriteMessageResult::Err(_) => break, // Unexpected error occurred
                }
            },
            ReadMessageResult::ConnectionClosed => break, // Connection closed, terminate
            ReadMessageResult::Err(_) => break, // Unexpected error occurred
        }
    }
}

fn handle_command(command_str: String, spreadsheet: &SharedSpreadsheet, thread_handles: ThreadHandles) -> Reply {
    let command: Command = match command_str.parse::<Command>() {
        Ok(command) => command,
        Err(e) => return Reply::Error(e.to_string()),
    };

    match command {
        Command::Get { cell_identifier } => {
            let spreadsheet = spreadsheet.lock().unwrap();
            let cell_id_str = cell_id_to_string(cell_identifier);

            let cell_value = spreadsheet
                .cells
                .get(&cell_id_str)
                .map(|timed_value| &timed_value.value)
                .unwrap_or(&CellValue::None);

            Reply::Value(cell_id_str, cell_value.clone())
        }
        Command::Set { cell_identifier, cell_expr } => {
            let cell_id_str = cell_id_to_string(cell_identifier);
            let cell_expr_str = cell_expr.clone();
        
            let cell_expr = CellExpr::new(&cell_expr_str);
            let cell_value: CellValue;

            {
                let mut spreadsheet = spreadsheet.lock().unwrap();

                let context: HashMap<String, CellArgument> = handle_context(&cell_expr, &spreadsheet.cells);
                let eval_value = cell_expr.evaluate(&context);

                cell_value = match eval_value {
                    Ok(value) => value,
                    Err(_) => return Reply::Error("could not evaluate expression".to_string()),
                };

                spreadsheet.cells.insert(
                    cell_id_str.clone(),
                    TimedCellValue {
                        value: cell_value.clone(),
                        expression: Some(cell_expr_str.clone()),
                        timestamp: Instant::now(),
                    }
                );

                update_dependencies(&mut spreadsheet.dependencies, &cell_id_str, &CellExpr::new(&cell_expr_str));
            }

            trigger_updates(Arc::clone(spreadsheet), cell_id_str.clone(), thread_handles);

            Reply::Value(cell_id_str, cell_value.clone())
        }
    }
}

/*
fn detect_cycle(dependencies: &DependencyGraph, cell_id: &str) -> bool {
    let mut visited = HashSet::new();
    let mut stack = vec![cell_id.to_string()];

    while let Some(current) = stack.pop() {
        if visited.contains(&current) {
            eprintln!("Cycle detected at cell: {}", current);
            return true;
        }
        visited.insert(current.clone());
        if let Some(dependents) = dependencies.get(&current) {
            stack.extend(dependents.clone());
        }
    }
    false
}
*/

fn update_dependencies(dependencies: &mut DependencyGraph, cell_id: &str, cell_expr: &CellExpr) {
    let vars = cell_expr.find_variable_names();

    // Remove `cell_id` from all current dependencies
    for deps in dependencies.values_mut() {
        deps.retain(|dep| dep != cell_id);
    }

    // Add `cell_id` as a dependent to all variables in `cell_expr`
    for var in vars {
        dependencies
            .entry(var)
            .or_insert_with(Vec::new)
            .push(cell_id.to_string());
    }

    /*
    if detect_cycle(dependencies, cell_id) {
        eprintln!("cycle detected after updating dependencies for cell: {}", cell_id);
        dependencies.remove(cell_id);
    }
    */
}

fn trigger_updates(shared_spreadsheet: SharedSpreadsheet, updated_cell: String, thread_handles: ThreadHandles) {
    let mut queue = vec![updated_cell];
    let mut visited = HashSet::new();

    while let Some(cell) = queue.pop() {
        if visited.contains(&cell) {
            continue; // Avoid processing the same cell multiple times
        }
        visited.insert(cell.clone());

        let mut spreadsheet = shared_spreadsheet.lock().unwrap();

        if let Some(dependents) = spreadsheet.dependencies.get(&cell).cloned() {
            for dependent in dependents {
                if let Some(original_expr) = spreadsheet.cells.get(&dependent) {
                    if let Some(original_expr_str) = &original_expr.expression {
                        let cloned_expression = original_expr.expression.clone();

                        let new_cell_expr = CellExpr::new(&original_expr_str);
                        let context = handle_context(&new_cell_expr, &spreadsheet.cells);

                        match new_cell_expr.evaluate(&context) {
                            Ok(new_value) => {
                                spreadsheet.cells.insert(
                                    dependent.clone(),
                                    TimedCellValue {
                                        value: new_value,
                                        expression: cloned_expression,
                                        timestamp: Instant::now(),
                                    },
                                );

                                queue.push(dependent.clone());
                            }
                            Err(_) => {
                                spreadsheet.cells.insert(
                                    dependent.clone(),
                                    TimedCellValue {
                                        value: CellValue::Error("evaluation failed".to_string()),
                                        expression: cloned_expression,
                                        timestamp: Instant::now(),
                                    },
                                );
                            }
                        }
                    } else {
                        println!("dependent {} has no valid expression to evaluate", dependent);
                    }
                }
            }
        }
    }
}

fn wait_for_threads(thread_handles: ThreadHandles) {
    let mut handles = thread_handles.lock().unwrap();
    for handle in handles.drain(..) {
        handle.join().expect("thread failed to join");
    }
}

fn handle_context(cell_expr: &CellExpr, cells: &CellGrid) -> HashMap<String, CellArgument> {
    let mut context: HashMap<String, CellArgument> = HashMap::new();
    let variables = cell_expr.find_variable_names();

    for var in variables {
        if let Some(cell_arg) = resolve_variable(&var, cells) {
            context.insert(var.clone(), cell_arg);
        }
    }

    context
}

fn resolve_variable(var_name: &str, cells: &CellGrid) -> Option<CellArgument> {
    if let Some(cell_value) = cells.get(var_name) {
        // Values
        return match &cell_value.value {
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
                .map(|cell| cells.get(cell).map(|timed| timed.value.clone())
                .unwrap_or(CellValue::None))
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

fn build_matrix(range: &[String], cells: &CellGrid) -> Vec<Vec<CellValue>> {
    let start_id = range.first().and_then(|c| c.parse::<CellIdentifier>().ok()).unwrap();
    let end_id = range.last().and_then(|c| c.parse::<CellIdentifier>().ok()).unwrap();

    let mut matrix = Vec::new();

    for row in start_id.row..=end_id.row {
        let mut row_values = Vec::new();
        for col in start_id.col..=end_id.col {
            let cell_id = CellIdentifier { col, row };
            let cell_name = cell_id_to_string(cell_id);
            let value = cells.get(&cell_name).cloned().map(|timed_cell| timed_cell.value.clone()).unwrap_or(CellValue::None);
            row_values.push(value);
        }
        matrix.push(row_values);
    }

    matrix
}
