//! The columns of All Orders, and the view that shows them the way the pages expect.
//!
//! The pages work with every cell as TEXT and with columns by number (the position in `column_defs`): "" for blank, "TRUE" / "FALSE"
//! for a checkbox, "2026-11-04" or "2026-11-04 15:05" for a date. The database keeps real types (dates, numbers, 1/0 ticks). The view
//! `orders_text` is the bridge: it shows the typed `orders` table as columns c0, c1, ... of text, exactly like the Python app's table,
//! so reading, filtering, sorting and counting work on it the same way. Writing goes to the real table (see `orders_write`).

use std::collections::HashMap;

use anyhow::Result;
use sqlx::MySqlPool;

/// The columns a search box looks in (headers).
pub const SEARCH: [&str; 13] = [
    "Label Number", "Store Order No.", "Product Name", "Brand", "Customer Name", "Address", "Source Order Number", "Source Tracking Number",
    "Tracking Number To Customer", "Notes", "PH Team Internal Notes", "SKU", "Label Chip",
];

/// Columns the Scan flow and the summary cards read by name: deleting one would break them.
const PROTECTED_SCAN: [&str; 24] = [
    "Label Number", "Store Order No.", "Product Name", "Brand", "QTY", "Customer Name", "Source Order Number", "Source Tracking Number", "Shipping Label Link",
    "Product Image Link", "Package Not Received", "Label Used", "Shipped to Customer", "Shipped to Amazon", "Label Voided", "Boxes Received", "Boxes Expected",
    "Hold Date", "Updated by", "SHIPPED TO  CUSTOMER DATE", "SHIPPED TO  AMAZON DATE", "Store", "Notes", "Items Received",
];
const PROTECTED_CARDS: [&str; 4] = ["Tracking Status", "ITEM ON HOLD", "To Void", "Stock"];

/// Calculated columns: the one on the left is always the columns on the right written one after the other.
pub const FORMULAS: [(&str, &[&str]); 1] = [("Gift Note", &["Label Number", "Customer Name"])];

#[derive(Clone, Debug)]
pub struct Col {
    /// The column number the pages use (the position in the schema; a deleted column keeps its number).
    pub i: usize,
    /// The name of the column in the `orders` table (label_number), or c_<n> for a column an admin added.
    pub field: String,
    /// What the program knows the column by ("Label Number"). Never changes.
    pub header: String,
    /// What the pages show (an admin may rename it).
    pub label: String,
    pub kind: String,
    pub custom: bool,
    pub deleted: bool,
    pub id: u64,
    /// An added column that is a dropdown: its choices.
    pub dropdown: bool,
    pub options: Vec<String>,
    /// The database type of a built-in column (tinyint, int, date, datetime, varchar, text ...).
    pub dtype: String,
}

#[derive(Clone, Default)]
pub struct Schema {
    pub cols: Vec<Col>,
    /// header -> column number, for the columns that are not deleted.
    pub ix: HashMap<String, usize>,
}

impl Schema {
    pub async fn load(db: &MySqlPool) -> Result<Self> {
        let types: HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
            "SELECT CAST(COLUMN_NAME AS CHAR), CAST(DATA_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'orders'",
        )
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();
        let rows: Vec<(u64, String, String, String, String, i8, i8, i8, Option<String>)> = sqlx::query_as(
            "SELECT CAST(id AS UNSIGNED), CAST(field AS CHAR), CAST(header AS CHAR), CAST(label AS CHAR), CAST(kind AS CHAR), custom, deleted, dropdown, CAST(options AS CHAR) FROM column_defs ORDER BY position, id",
        )
        .fetch_all(db)
        .await?;
        let cols: Vec<Col> = rows
            .into_iter()
            .enumerate()
            .map(|(i, (id, field, header, label, kind, custom, deleted, dropdown, options))| Col {
                dropdown: dropdown != 0,
                options: options.and_then(|o| serde_json::from_str(&o).ok()).unwrap_or_default(),
                i,
                dtype: types.get(&field).cloned().unwrap_or_default(),
                field,
                header,
                label,
                kind,
                custom: custom != 0,
                deleted: deleted != 0,
                id,
            })
            .collect();
        let ix = cols.iter().filter(|c| !c.deleted).map(|c| (c.header.clone(), c.i)).collect();
        Ok(Self { cols, ix })
    }

    pub fn n(&self) -> usize {
        self.cols.len()
    }

    pub fn active(&self, i: usize) -> bool {
        self.cols.get(i).is_some_and(|c| !c.deleted)
    }

    /// The column number of a header, for a column that exists.
    pub fn at(&self, header: &str) -> Option<usize> {
        self.ix.get(header).copied()
    }

    /// `c12`-style name of a header's column in the text view. Panics are avoided: a missing column answers with a column that is always empty.
    pub fn c(&self, header: &str) -> String {
        self.at(header).map_or_else(|| "''".to_string(), |i| format!("c{i}"))
    }

    pub fn search_idx(&self) -> Vec<usize> {
        SEARCH.iter().filter_map(|h| self.at(h)).collect()
    }

    pub fn protected(&self, header: &str) -> &'static str {
        if PROTECTED_SCAN.contains(&header) {
            "The Scan page reads it"
        } else if PROTECTED_CARDS.contains(&header) {
            "A summary card on All Orders counts it"
        } else {
            ""
        }
    }

    pub fn formula_cols(&self) -> Vec<usize> {
        FORMULAS.iter().filter(|(t, src)| self.at(t).is_some() && src.iter().all(|s| self.at(s).is_some())).filter_map(|(t, _)| self.at(t)).collect()
    }

    /// The SQL of one column as text, for the view (`o` is the orders table).
    fn text_expr(&self, c: &Col) -> String {
        if c.deleted {
            return "''".into();
        }
        if c.custom {
            return format!("COALESCE((SELECT v.`value` FROM order_custom_values v WHERE v.rn = o.rn AND v.column_id = {}), '')", c.id);
        }
        let f = format!("o.`{}`", c.field);
        match c.dtype.as_str() {
            "tinyint" => format!("CASE {f} WHEN 1 THEN 'TRUE' WHEN 0 THEN 'FALSE' ELSE '' END"),
            "int" | "bigint" | "smallint" | "decimal" => format!("COALESCE(CAST({f} AS CHAR), '')"),
            "date" => format!("COALESCE(DATE_FORMAT({f}, '%Y-%m-%d'), '')"),
            // a time of exactly 00:00 is shown as a date only (the old app kept such values without a time)
            "datetime" => format!("COALESCE(IF(TIME({f}) = '00:00:00', DATE_FORMAT({f}, '%Y-%m-%d'), DATE_FORMAT({f}, '%Y-%m-%d %H:%i')), '')"),
            _ => format!("COALESCE({f}, '')"),
        }
    }

    /// (Re)creates the text view. Run at start-up and whenever the columns change.
    pub async fn make_view(&self, db: &MySqlPool) -> Result<()> {
        let cols: Vec<String> = self.cols.iter().map(|c| format!("{} AS c{}", self.text_expr(c), c.i)).collect();
        let list = if cols.is_empty() { String::new() } else { format!("{}, ", cols.join(", ")) };
        let sql = format!("CREATE OR REPLACE VIEW orders_text AS SELECT o.rn AS rn, {list}COALESCE(o.search_text, '') AS _s FROM orders o");
        sqlx::query(&sql).execute(db).await?;
        Ok(())
    }
}
