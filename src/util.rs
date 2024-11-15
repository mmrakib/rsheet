// rsheet_lib imports
use rsheet_lib::cells::{column_name_to_number, column_number_to_name};
use rsheet_lib::command::CellIdentifier;

/// Converts a cell ID to a string
///
/// # Arguments
/// * `cell_id` - The `CellIdentifier` object to convert
/// 
/// # Returns
/// The converted `String` representation of the `CellIdentifier` object
pub fn cell_id_to_string(cell_id: CellIdentifier) -> String {
    let CellIdentifier { col, row } = cell_id;
    let col_name = column_number_to_name(col);

    format!("{}{}", col_name, row + 1)
}
