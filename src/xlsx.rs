//! A small .xlsx reader: sheet names and rows as lists of strings.
//!
//! Dates stored as real Excel dates come back as ISO text ("2026-03-06" or "2026-03-06 14:30"), booleans as TRUE / FALSE, everything
//! else as the text Excel shows for the stored value. (The same rules as the Python app's reader.)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufReader, Read, Seek};

use anyhow::{anyhow, Result};
use chrono::{Duration, NaiveDate};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use regex::Regex;
use zip::ZipArchive;

use crate::amazon::plain_number;

pub struct Validation {
    pub kind: String,
    pub sqref: String,
    pub formula: String,
    pub strict: bool,
}

pub struct Workbook<R: Read + Seek> {
    zip: ZipArchive<R>,
    shared: Option<Vec<String>>,
    date_xf: Option<HashSet<usize>>,
    pub validations: Vec<Validation>,
}

/// "AB" -> 27 (the column index, from 0).
pub fn col_index(letters: &str) -> usize {
    letters.bytes().fold(0usize, |n, b| n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize) - 1
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == name.as_bytes()).and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn is_date_format(code: &str) -> bool {
    let cleaned = Regex::new(r#""[^"]*"|\[[^\]]*\]|\\."#).unwrap().replace_all(code, "");
    Regex::new(r"(?i)[ymdhs]").unwrap().is_match(&cleaned)
}

const BUILTIN_DATE_IDS: [(usize, usize); 4] = [(14, 22), (27, 36), (45, 47), (50, 58)];

impl<R: Read + Seek> Workbook<R> {
    pub fn new(r: R) -> Result<Self> {
        Ok(Self { zip: ZipArchive::new(r)?, shared: None, date_xf: None, validations: vec![] })
    }

    fn read_entry(&mut self, name: &str) -> Result<Vec<u8>> {
        let mut f = self.zip.by_name(name).map_err(|_| anyhow!("{name} is not in the file"))?;
        let mut v = vec![];
        f.read_to_end(&mut v)?;
        Ok(v)
    }

    /// [(name, path inside the zip)] in tab order.
    pub fn sheets(&mut self) -> Result<Vec<(String, String)>> {
        let wb = self.read_entry("xl/workbook.xml")?;
        let rels = self.read_entry("xl/_rels/workbook.xml.rels")?;
        let mut targets: HashMap<String, String> = HashMap::new();
        let mut rd = Reader::from_reader(rels.as_slice());
        let mut buf = vec![];
        loop {
            match rd.read_event_into(&mut buf)? {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                    if let (Some(id), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                        targets.insert(id, t);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        let mut out = vec![];
        let mut rd = Reader::from_reader(wb.as_slice());
        loop {
            match rd.read_event_into(&mut buf)? {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"sheet" => {
                    let name = attr(&e, "name").unwrap_or_default();
                    let id = attr(&e, "id").unwrap_or_default();
                    let target = targets.get(&id).cloned().unwrap_or_default();
                    let target = target.trim_start_matches('/').to_string();
                    out.push((name, if target.starts_with("xl/") { target } else { format!("xl/{target}") }));
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }

    fn shared(&mut self) -> Result<&Vec<String>> {
        if self.shared.is_none() {
            let mut list = vec![];
            if let Ok(data) = self.read_entry("xl/sharedStrings.xml") {
                let mut rd = Reader::from_reader(data.as_slice());
                let (mut buf, mut cur, mut in_si, mut in_t) = (vec![], String::new(), false, false);
                loop {
                    match rd.read_event_into(&mut buf)? {
                        Event::Start(e) => match e.local_name().as_ref() {
                            b"si" => (in_si, cur) = (true, String::new()),
                            b"t" if in_si => in_t = true,
                            _ => {}
                        },
                        Event::Text(t) if in_t => cur.push_str(&t.unescape()?),
                        Event::End(e) => match e.local_name().as_ref() {
                            b"t" => in_t = false,
                            b"si" => {
                                in_si = false;
                                list.push(std::mem::take(&mut cur));
                            }
                            _ => {}
                        },
                        Event::Eof => break,
                        _ => {}
                    }
                    buf.clear();
                }
            }
            self.shared = Some(list);
        }
        Ok(self.shared.as_ref().unwrap())
    }

    fn date_styles(&mut self) -> Result<&HashSet<usize>> {
        if self.date_xf.is_none() {
            let mut set = HashSet::new();
            if let Ok(data) = self.read_entry("xl/styles.xml") {
                let mut rd = Reader::from_reader(data.as_slice());
                let (mut buf, mut custom, mut in_xfs, mut i) = (vec![], HashMap::<usize, String>::new(), false, 0usize);
                loop {
                    match rd.read_event_into(&mut buf)? {
                        Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                            b"numFmt" => {
                                if let (Some(id), Some(code)) = (attr(&e, "numFmtId").and_then(|v| v.parse().ok()), attr(&e, "formatCode")) {
                                    custom.insert(id, code);
                                }
                            }
                            b"cellXfs" => in_xfs = true,
                            b"xf" if in_xfs => {
                                let fid: usize = attr(&e, "numFmtId").and_then(|v| v.parse().ok()).unwrap_or(0);
                                if BUILTIN_DATE_IDS.iter().any(|(a, b)| (*a..=*b).contains(&fid)) || custom.get(&fid).is_some_and(|c| is_date_format(c)) {
                                    set.insert(i);
                                }
                                i += 1;
                            }
                            _ => {}
                        },
                        Event::End(e) if e.local_name().as_ref() == b"cellXfs" => in_xfs = false,
                        Event::Eof => break,
                        _ => {}
                    }
                    buf.clear();
                }
            }
            self.date_xf = Some(set);
        }
        Ok(self.date_xf.as_ref().unwrap())
    }

    /// The non-empty rows of a sheet as (row number, cells). Dropdown rules found in the sheet are added to `validations`.
    /// `tick(fraction)` is called now and then with how much of the sheet's XML has been read (0 to 1).
    pub fn rows(&mut self, path: &str, limit: Option<usize>, mut tick: Option<&mut dyn FnMut(f64)>) -> Result<Vec<(usize, Vec<String>)>> {
        let shared = self.shared()?.clone();
        let date_xf = self.date_styles()?.clone();
        let size = self.zip.by_name(path).map_err(|_| anyhow!("the sheet is not in the file"))?.size().max(1);
        let entry = self.zip.by_name(path)?;
        let mut rd = Reader::from_reader(BufReader::new(entry));
        let mut buf = vec![];
        let mut out: Vec<(usize, Vec<String>)> = vec![];
        let mut n = 0usize;
        let mut cells: BTreeMap<usize, String> = BTreeMap::new();
        let mut row_no = 0usize;
        // the cell being read
        let (mut col, mut ty, mut style, mut v, mut inline, mut in_v, mut in_is, mut in_t) = (0usize, String::new(), 0usize, None::<String>, String::new(), false, false, false);
        let (mut dv, mut in_f1) = (None::<Validation>, false);
        loop {
            match rd.read_event_into(&mut buf)? {
                Event::Start(e) => match e.local_name().as_ref() {
                    b"row" => {
                        row_no = attr(&e, "r").and_then(|r| r.parse().ok()).filter(|r| *r != 0).unwrap_or(n + 1);
                        cells.clear();
                    }
                    b"c" => {
                        let r = attr(&e, "r").unwrap_or_default();
                        col = col_index(r.trim_end_matches(|c: char| c.is_ascii_digit()));
                        ty = attr(&e, "t").unwrap_or_default();
                        style = attr(&e, "s").and_then(|s| s.parse().ok()).unwrap_or(0);
                        (v, inline) = (None, String::new());
                    }
                    b"v" => in_v = true,
                    b"is" => in_is = true,
                    b"t" if in_is => in_t = true,
                    b"dataValidation" => {
                        dv = Some(Validation {
                            kind: attr(&e, "type").unwrap_or_default(),
                            sqref: attr(&e, "sqref").unwrap_or_default(),
                            formula: String::new(),
                            strict: attr(&e, "showErrorMessage").as_deref() == Some("1") && attr(&e, "errorStyle").as_deref().unwrap_or("stop") == "stop",
                        })
                    }
                    b"formula1" if dv.is_some() => in_f1 = true,
                    _ => {}
                },
                Event::Text(t) => {
                    let text = t.unescape()?;
                    if in_v {
                        v = Some(text.to_string());
                    } else if in_t {
                        inline.push_str(&text);
                    } else if in_f1 {
                        if let Some(d) = dv.as_mut() {
                            d.formula.push_str(&text);
                        }
                    }
                }
                Event::End(e) => match e.local_name().as_ref() {
                    b"v" => in_v = false,
                    b"t" => in_t = false,
                    b"is" => in_is = false,
                    b"formula1" => in_f1 = false,
                    b"dataValidation" => {
                        if let Some(d) = dv.take() {
                            self.validations.push(d);
                        }
                    }
                    b"c" => {
                        let value = if ty == "inlineStr" {
                            Some(inline.clone())
                        } else if let Some(txt) = v.take() {
                            Some(match ty.as_str() {
                                "s" => txt.parse::<usize>().ok().and_then(|i| shared.get(i).cloned()).unwrap_or_default(),
                                "b" => if txt == "1" { "TRUE".into() } else { "FALSE".into() },
                                "e" => String::new(),
                                "str" | "d" => txt,
                                _ if date_xf.contains(&style) => match txt.parse::<f64>() {
                                    Ok(days) => {
                                        let micros = (days * 86_400_000_000.0).round() as i64;
                                        let d = NaiveDate::from_ymd_opt(1899, 12, 30).unwrap().and_hms_opt(0, 0, 0).unwrap() + Duration::microseconds(micros);
                                        if d.format("%H%M").to_string() != "0000" { d.format("%Y-%m-%d %H:%M").to_string() } else { d.format("%Y-%m-%d").to_string() }
                                    }
                                    Err(_) => txt,
                                },
                                _ => plain_number(txt.strip_suffix(".0").unwrap_or(&txt)),
                            })
                        } else {
                            None
                        };
                        if let Some(val) = value {
                            cells.insert(col, val);
                        }
                    }
                    b"row" => {
                        n = row_no;
                        if !cells.is_empty() {
                            let width = cells.keys().max().map_or(0, |m| m + 1);
                            let mut cols = vec![String::new(); width];
                            for (i, val) in std::mem::take(&mut cells) {
                                cols[i] = val;
                            }
                            out.push((row_no, cols));
                            if limit.is_some_and(|l| row_no >= l) {
                                return Ok(out);
                            }
                        }
                        if row_no % 500 == 0 {
                            if let Some(t) = tick.as_mut() {
                                t((rd.buffer_position() as f64 / size as f64).min(1.0));
                            }
                        }
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }
}
