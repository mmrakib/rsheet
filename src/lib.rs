mod util;

// rsheet_lib imports
use rsheet_lib::command::{Command, CellIdentifier};
use rsheet_lib::connect::{Connection, Manager, Reader, Writer, ReadMessageResult, WriteMessageResult};
use rsheet_lib::replies::Reply;
use rsheet_lib::cell_expr::{CellExpr, CellArgument};
use rsheet_lib::cell_value::CellValue;

// External imports
use log::info;

use std::cell;
// Standard lib imports
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::error::Error;

// Internal imports
use crate::util::cell_id_to_string;

pub fn start_server<M>(mut manager: M) -> Result<(), Box<dyn Error>>
where
    M: Manager,
{
    // Cell grid
    let cells: Arc<Mutex< HashMap<String, CellValue> >> = Arc::new(Mutex::new(HashMap::new()));

    loop {
        // Initiate client connection
        let (mut recv, mut send) = match manager.accept_new_connection() {
            Connection::NewConnection { reader, writer } => (reader, writer),
            Connection::NoMoreConnections => return Ok(()), // No more new connections, terminate
        };

        loop {
            // Read request message from client
            let message: ReadMessageResult = recv.read_message();
            info!("Message received");

            match message {
                ReadMessageResult::Message(msg) => {
                    // Handle command and get reply
                    let reply = handle_command(msg, &cells);
    
                    // Write reply message to client
                    match send.write_message(reply) {
                        WriteMessageResult::Ok => continue, // Message sent successfully
                        WriteMessageResult::ConnectionClosed => break, // Connection closed, terminate
                        WriteMessageResult::Err(e) => return Err(Box::new(e)), // Unexpected error occurred
                    }
                },
                ReadMessageResult::ConnectionClosed => break, // Connection closed, terminate
                ReadMessageResult::Err(e) => return Err(Box::new(e)), // Unexpected error occurred
            }
        }
    }
}

fn handle_command(command_str: String, cells: &Arc<Mutex< HashMap<String, CellValue> >>) -> Reply {
    let command: Command = match command_str.parse::<Command>() {
        Ok(command) => command,
        Err(e) => return Reply::Error(e.to_string()),
    };

    match command {
        Command::Get { cell_identifier } => {
            let cell_id_str = cell_id_to_string(cell_identifier);

            let cells = cells.lock().unwrap();
            let cell_value = cells.get(&cell_id_str).unwrap_or(&CellValue::None);

            Reply::Value(cell_id_str, cell_value.clone())
        }
        Command::Set { cell_identifier, cell_expr } => {
            let cell_id_str = cell_id_to_string(cell_identifier);

            let cell_expr = CellExpr::new(&cell_expr);

            let variables: HashMap<String, CellArgument> = HashMap::new();
            let eval_value = cell_expr.evaluate(&variables);

            let cell_value = match eval_value {
                Ok(value) => value,
                Err(_) => return Reply::Error("could not evaluate expression".to_string()),
            };

            let mut cells = cells.lock().unwrap();
            cells.insert(cell_id_str, cell_value.clone());

            Reply::Value(cell_id_to_string(cell_identifier), cell_value.clone())
        }
    }
}
