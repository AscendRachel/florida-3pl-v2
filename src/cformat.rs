//! Colours of the conditional formatting, worked out in SQL, so All Orders can sort and filter by colour (like Google Sheets).
//!
//! This mirrors web/static/js/cond-format.js, which paints the cells (and the Python app's cformat.py). Keep them in step: a rule
//! changed there must be changed here. A colour is a hex like "#00ffff", "dup" for the duplicate colour (black or white by theme),
//! or "" for no colour.

use crate::schema::Schema;

/// The columns that the status colours paint across the row (plus Label Chip).
const ROW_COLS: [&str; 30] = [
    "Label Number", "Gift Note", "Store Order No.", "Product Name", "Brand", "QTY", "Customer Name", "Address", "Source Order Date", "Source Order Number",
    "Source Tracking Number", "Shipping Label Link", "Product Image Link", "Source Order Status", "Package Not Received", "To Void", "ITEM ON HOLD", "Stock",
    "Label Used", "Shipped to Customer", "Shipped to Amazon", "Label Voided", "Tracking Number To Customer", "Delivery Date", "Tracking Status", "Notes",
    "Store", "PH Team Internal Notes", "Days In Transit", "Label Chip",
];
pub const DUP_COLUMNS: [&str; 4] = ["Store Order No.", "Source Order Number", "Shipping Label Link", "Tracking Number To Customer"];

pub const NAMES: [(&str, &str); 15] = [
    ("dup", "Duplicate"), ("#00ffff", "Cyan"), ("#c3b091", "Tan"), ("#9900ff", "Purple"), ("#ff9900", "Orange"), ("#f4cccc", "Light pink"), ("#783f04", "Brown"),
    ("#ff00ff", "Magenta"), ("#ffff00", "Yellow"), ("#b7b7b7", "Grey"), ("#6aa84f", "Green"), ("#6d9eeb", "Blue"), ("#00ff00", "Bright green"), ("#ff0000", "Red"),
    ("#b7e1cd", "Light green"),
];

pub fn color_name(hex: &str) -> &'static str {
    NAMES.iter().find(|(h, _)| *h == hex).map_or("", |(_, n)| n)
}

fn eq(col: &str, text: &str) -> String {
    format!("lower(trim({col})) = '{}'", text.to_lowercase())
}

/// The SQL expression (one text value per row) for the fill colour of the column with this header, or "''" when no rule can colour it.
/// `today` is 'YYYY-MM-DD' and `day_ago` 'YYYY-MM-DD HH:MM', both US Eastern.
pub fn fill_sql(schema: &Schema, header: &str, today: &str, day_ago: &str) -> String {
    let c = |n: &str| schema.c(n);
    let mut whens: Vec<String> = vec![];
    if DUP_COLUMNS.contains(&header) {
        let col = c(header);
        // The values that occur more than once are looked up beforehand (see `dup_values`) and written in as a plain list: asking the
        // database for them inside this expression made it repeat the search for every order, so a list of 78,000 rows never came back.
        match dup_values(header) {
            Some(list) if list.is_empty() => {}
            Some(list) => {
                let (bs, q) = (char::from(92u8), char::from(39u8));
                let lits: Vec<String> = list.iter().map(|v| format!("{q}{}{q}", v.replace(bs, &format!("{bs}{bs}")).replace(q, &format!("{q}{q}")).replace(char::from(0u8), ""))).collect();
                whens.push(format!("WHEN trim({col}) <> '' AND lower(trim({col})) IN ({}) THEN 'dup'", lits.join(",")));
            }
            None => whens.push(format!(
                "WHEN trim({col}) <> '' AND lower(trim({col})) IN (SELECT v FROM (SELECT lower(trim({col})) v FROM orders_text WHERE trim({col}) <> '' GROUP BY 1 HAVING COUNT(*) > 1) d) THEN 'dup'"
            )),
        }
    }
    if ROW_COLS.contains(&header) {
        let rules: Vec<(String, &str)> = vec![
            (eq(&c("Source Order Status"), "refunded"), "#00ffff"),
            (eq(&c("Tracking Status"), "store returns"), "#c3b091"),
            (eq(&c("Tracking Status"), "return to sender"), "#9900ff"),
            (format!("upper(trim({})) = 'TRUE'", c("Package Not Received")), "#ff9900"),
            (eq(&c("Source Order Status"), "for refund"), "#f4cccc"),
            (eq(&c("Tracking Status"), "return to amazon"), "#783f04"),
            (format!("upper(trim({})) = 'TRUE' AND {}", c("Shipped to Customer"), eq(&c("Tracking Status"), "preshipment")), "#ff00ff"),
            (eq(&c("Tracking Status"), "delivered"), "#ffff00"),
            (eq(&c("Tracking Status"), "in transit"), "#b7b7b7"),
            (eq(&c("Source Order Status"), "unfulfilled"), "#6aa84f"),
            (eq(&c("Source Order Status"), "cancelled"), "#6d9eeb"),
            (eq(&c("Source Order Status"), "delivered"), "#00ff00"),
            (
                format!(
                    "trim({}) <> '' AND (trim({d}) = '' OR substr({d}, 1, 10) < '{today}') AND lower(trim({})) <> 'delivered'",
                    c("Source Order Number"),
                    c("Source Order Status"),
                    d = c("Delivery Date")
                ),
                "#ff0000",
            ),
        ];
        for (cond, colour) in rules {
            whens.push(format!("WHEN {cond} THEN '{colour}'"));
        }
    }
    match header {
        "Source Tracking Number" => whens.push(format!("WHEN trim({}) <> '' THEN '#b7e1cd'", c(header))),
        "Hold Date" => whens.push(format!("WHEN upper(trim({})) = 'TRUE' AND {h} <> '' AND substr({h}, 1, 16) < '{day_ago}' THEN '#ff0000'", c("ITEM ON HOLD"), h = c(header))),
        "TAPICKER SHIPPED DATE" | "TAPICKER DELIVERY DATE" => {
            let (s, d) = (c("TAPICKER SHIPPED DATE"), c("TAPICKER DELIVERY DATE"));
            whens.push(format!("WHEN {s} <> '' AND {d} <> '' AND substr({s}, 1, 16) > substr({d}, 1, 16) THEN '#ff0000'"));
        }
        "Label Chip" => {
            // the sheet's NOT(REGEXMATCH(LOWER(chip), LOWER(SUBSTITUTE(name, " ", ".*")))): spaces in the name match anything.
            // Anything in the name that is not a letter, digit or space matches any one character, so an odd name can never break the query.
            let (chip, who) = (c(header), c("Customer Name"));
            whens.push(format!(
                "WHEN {chip} <> '' AND {who} <> '' AND NOT (LOWER({chip}) REGEXP REPLACE(REGEXP_REPLACE(LOWER({who}), '[^a-z0-9 ]', '.'), ' ', '.*')) THEN '#ff0000'"
            ));
        }
        _ => {}
    }
    if whens.is_empty() {
        "''".to_string()
    } else {
        format!("(CASE {} ELSE '' END)", whens.join(" "))
    }
}

/// The values of each duplicate-checked column that occur more than once (lower-cased, trimmed), with the data version they were read at.
static DUPS: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, std::sync::Arc<Vec<String>>)>>> = std::sync::Mutex::new(None);
/// Most values that are written into a query as a list; more than that and the query asks the database instead.
pub const DUP_LIST_MAX: usize = 30_000;

pub fn dup_values(header: &str) -> Option<std::sync::Arc<Vec<String>>> {
    DUPS.lock().unwrap().as_ref()?.get(header).map(|(_, l)| l.clone())
}
pub fn dup_version(header: &str) -> Option<u64> {
    DUPS.lock().unwrap().as_ref()?.get(header).map(|(v, _)| *v)
}
pub fn set_dup_values(header: &str, version: u64, list: Vec<String>) {
    let mut g = DUPS.lock().unwrap();
    let m = g.get_or_insert_with(Default::default);
    if list.len() > DUP_LIST_MAX {
        m.remove(header);
    } else {
        m.insert(header.to_string(), (version, std::sync::Arc::new(list)));
    }
}
