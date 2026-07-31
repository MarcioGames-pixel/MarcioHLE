use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::mem::{ConstPtr, MutPtr};
use crate::Environment;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Mutex;

// SQLite result codes per https://sqlite.org
const SQLITE_OK: u32 = 0;
const SQLITE_ERROR: u32 = 1;
const SQLITE_ROW: u32 = 100;
const SQLITE_DONE: u32 = 101;
const SQLITE_MISUSE: u32 = 21;

// SQLite Open Flags for open_v2 mapping
const SQLITE_OPEN_READONLY: u32 = 0x00000001;
const SQLITE_OPEN_READWRITE: u32 = 0x00000002;
const SQLITE_OPEN_CREATE: u32 = 0x00000004;

lazy_static::lazy_static! {
    static ref SQLITE_CONNECTIONS: Mutex<HashMap<u32, Connection>> = Mutex::new(HashMap::new());
    static ref SQLITE_STATEMENTS: Mutex<HashMap<u32, StmtEntry>> = Mutex::new(HashMap::new());
    static ref NEXT_HANDLE: Mutex<u32> = Mutex::new(0x8000_0000);
    static ref LAST_ERROR: Mutex<HashMap<u32, String>> = Mutex::new(HashMap::new());
}

struct StmtEntry {
    db_handle: u32,
    sql: String,
    bindings: HashMap<usize, BindValue>,
    columns: Vec<ColumnValue>,
    column_names: Vec<String>,
    column_name_ptrs: HashMap<i32, u32>,
    done: bool,
}

#[derive(Clone)]
enum BindValue {
    Int(i32),
    Int64(i64),
    Double(f64),
    Text(String),
    Blob(Vec<u8>),
    Null,
}

#[derive(Clone)]
enum ColumnValue {
    Int(i64),
    Double(f64),
    Text(String),
    Blob(Vec<u8>),
    Null,
}

fn read_cstring(env: &Environment, ptr: u32) -> String {
    if ptr == 0 {
        return String::new();
    }
    let mut bytes = Vec::new();
    let mut addr = ptr;
    loop {
        let p: ConstPtr<u8> = ConstPtr::from_bits(addr);
        let b: u8 = env.mem.read(p);
        if b == 0 {
            break;
        }
        bytes.push(b);
        addr += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn alloc_handle() -> u32 {
    let mut next = NEXT_HANDLE.lock().unwrap();
    let h = *next;
    *next += 4;
    h
}

fn set_error(db_handle: u32, msg: String) {
    LAST_ERROR.lock().unwrap().insert(db_handle, msg);
}

// ---------- sqlite3_open ----------
pub fn sqlite3_open(env: &mut Environment, filename_ptr: u32, pp_db: u32) -> u32 {
    let filename = read_cstring(env, filename_ptr);

    let path = if filename == ":memory:" {
        ":memory:".to_string()
    } else {
        let safe_name = filename
            .replace('/', "_")
            .replace('\\', "_")
            .replace(':', "_");

        let app_ns = std::env::var("TOUCHHLE_SQLITE_NAMESPACE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| env.bundle.bundle_identifier().to_string())
            .replace('/', "_")
            .replace('\\', "_")
            .replace(':', "_")
            .replace(' ', "_");

        let dir = crate::paths::user_data_base_path().join("touchHLE_sqlite");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log!("libsqlite3: failed to create sqlite dir {:?}: {}", dir, e);
        }

        dir.join(format!("{}_{}", app_ns, safe_name))
            .to_string_lossy()
            .into_owned()
    };

    log!("libsqlite3: sqlite3_open requested {:?} => {:?}", filename, path);

    match Connection::open(&path) {
        Ok(conn) => {
            let handle = alloc_handle();
            SQLITE_CONNECTIONS.lock().unwrap().insert(handle, conn);
            let p: MutPtr<u32> = MutPtr::from_bits(pp_db);
            env.mem.write(p, handle);
            SQLITE_OK
        }
        Err(e) => {
            log!("libsqlite3: sqlite3_open failed for {:?}: {}", path, e);
            let p: MutPtr<u32> = MutPtr::from_bits(pp_db);
            env.mem.write(p, 0u32);
            SQLITE_ERROR
        }
    }
}

// ---------- sqlite3_open_v2 ----------
pub fn sqlite3_open_v2(
    env: &mut Environment,
    filename_ptr: u32,
    pp_db: u32,
    flags: u32,
    _vfs: u32,
) -> u32 {
    let filename = read_cstring(env, filename_ptr);

    let path = if filename == ":memory:" {
        ":memory:".to_string()
    } else {
        let safe_name = filename
            .replace('/', "_")
            .replace('\\', "_")
            .replace(':', "_");

        let app_ns = std::env::var("TOUCHHLE_SQLITE_NAMESPACE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| env.bundle.bundle_identifier().to_string())
            .replace('/', "_")
            .replace('\\', "_")
            .replace(':', "_")
            .replace(' ', "_");

        let dir = crate::paths::user_data_base_path().join("touchHLE_sqlite");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log!("libsqlite3: failed to create sqlite dir {:?}: {}", dir, e);
        }

        dir.join(format!("{}_{}", app_ns, safe_name))
            .to_string_lossy()
            .into_owned()
    };

    log!(
        "libsqlite3: sqlite3_open_v2 requested {:?} with flags {:#x} => {:?}",
        filename, flags, path
    );

    // Mapeamento correto de OpenFlags para evitar conflitos de leitura/escrita
    let mut rusqlite_flags = rusqlite::OpenFlags::empty();
    if (flags & SQLITE_OPEN_READONLY) != 0 {
        rusqlite_flags.insert(rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY);
    }
    if (flags & SQLITE_OPEN_READWRITE) != 0 {
        rusqlite_flags.insert(rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE);
    }
    if (flags & SQLITE_OPEN_CREATE) != 0 {
        rusqlite_flags.insert(rusqlite::OpenFlags::SQLITE_OPEN_CREATE);
    }
    if rusqlite_flags.is_empty() {
        rusqlite_flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE;
    }

    match Connection::open_with_flags(&path, rusqlite_flags) {
        Ok(conn) => {
            let handle = alloc_handle();
            SQLITE_CONNECTIONS.lock().unwrap().insert(handle, conn);
            let p: MutPtr<u32> = MutPtr::from_bits(pp_db);
            env.mem.write(p, handle);
            SQLITE_OK
        }
        Err(e) => {
            log!("libsqlite3: sqlite3_open_v2 failed for {:?}: {}", path, e);
            let p: MutPtr<u32> = MutPtr::from_bits(pp_db);
            env.mem.write(p, 0u32);
            SQLITE_ERROR
        }
    }
}

// ---------- sqlite3_close ----------
pub fn sqlite3_close(_env: &mut Environment, p_db: u32) -> u32 {
    let mut handles = SQLITE_CONNECTIONS.lock().unwrap();
    if handles.remove(&p_db).is_some() {
        LAST_ERROR.lock().unwrap().remove(&p_db);
        SQLITE_OK
    } else {
        SQLITE_ERROR
    }
}

pub fn sqlite3_close_v2(_env: &mut Environment, p_db: u32) -> u32 {
    let mut handles = SQLITE_CONNECTIONS.lock().unwrap();
    if handles.remove(&p_db).is_some() {
        LAST_ERROR.lock().unwrap().remove(&p_db);
        SQLITE_OK
    } else {
        SQLITE_ERROR
    }
}

// ---------- sqlite3_exec ----------
pub fn sqlite3_exec(
    env: &mut Environment,
    p_db: u32,
    sql_ptr: u32,
    _callback: u32,
    _user_data: u32,
    err_msg_ptr: u32,
) -> u32 {
    let sql = read_cstring(env, sql_ptr);
    log!("libsqlite3: sqlite3_exec: {}", &sql[..sql.len().min(120)]);

    let handles = SQLITE_CONNECTIONS.lock().unwrap();
    let conn = match handles.get(&p_db) {
        Some(c) => c,
        None => {
            set_error(p_db, "no such connection".into());
            return SQLITE_ERROR;
        }
    };

    match conn.execute_batch(&sql) {
        Ok(_) => {
            if err_msg_ptr != 0 {
                let p: MutPtr<u32> = MutPtr::from_bits(err_msg_ptr);
                env.mem.write(p, 0u32);
            }
            SQLITE_OK
        }
        Err(e) => {
            let msg = format!("{}", e);
            log!("libsqlite3: sqlite3_exec error: {}", msg);
            set_error(p_db, msg);
            if err_msg_ptr != 0 {
                let p: MutPtr<u32> = MutPtr::from_bits(err_msg_ptr);
                env.mem.write(p, 0u32);
            }
            SQLITE_ERROR
        }
    }
}

// ---------- sqlite3_prepare (legacy V1) ----------
pub fn sqlite3_prepare(
    env: &mut Environment,
    p_db: u32,
    sql_ptr: u32,
    n_byte: i32,
    pp_stmt: u32,
    pp_tail: u32,
) -> u32 {
    sqlite3_prepare_v2(env, p_db, sql_ptr, n_byte, pp_stmt, pp_tail)
}

// ---------- sqlite3_prepare_v2 ----------
pub fn sqlite3_prepare_v2(
    env: &mut Environment,
    p_db: u32,
    sql_ptr: u32,
    n_byte: i32, // Removido o sublinhado para avaliar o limite de bytes da query
    pp_stmt: u32,
    _pp_tail: u32,
) -> u32 {
    let mut sql = read_cstring(env, sql_ptr);

    // CORREÇÃO CRÍTICA: Se n_byte > 0, trunca a string para obedecer ao buffer exato passado pela Unity
    if n_byte > 0 && (sql.len() > n_byte as usize) {
        sql.truncate(n_byte as usize);
    }

    log!(
        "libsqlite3: sqlite3_prepare_v2: {}",
        &sql[..sql.len().min(120)]
    );

    {
        let handles = SQLITE_CONNECTIONS.lock().unwrap();
        if !handles.contains_key(&p_db) {
            set_error(p_db, "no such connection".into());
            if pp_stmt != 0 {
                let p: MutPtr<u32> = MutPtr::from_bits(pp_stmt);
                env.mem.write(p, 0u32);
            }
            return SQLITE_ERROR;
        }
    }

    let valid = {
        let handles = SQLITE_CONNECTIONS.lock().unwrap();
        let conn = handles.get(&p_db).unwrap();
        conn.prepare(&sql).is_ok()
    };

    if !valid {
        let handles = SQLITE_CONNECTIONS.lock().unwrap();
        let conn = handles.get(&p_db).unwrap();
        let err = conn.prepare(&sql).unwrap_err();
        let msg = format!("{}", err);
        log!("libsqlite3: prepare error: {}", msg);
        set_error(p_db, msg);
        if pp_stmt != 0 {
            let p: MutPtr<u32> = MutPtr::from_bits(pp_stmt);
            env.mem.write(p, 0u32);
        }
        return SQLITE_ERROR;
    }

    let stmt_handle = alloc_handle();
    SQLITE_STATEMENTS.lock().unwrap().insert(
        stmt_handle,
        StmtEntry {
            db_handle: p_db,sql,bindings: HashMap::new(),columns: Vec::new(),column_names: Vec::new(),column_name_ptrs: HashMap::new(),done: false,},);if pp_stmt != 0 {let p: MutPtr = MutPtr::from_bits(pp_stmt);env.mem.write(p, stmt_handle);}SQLITE_OK}


    // Valida se a conexão com o banco de dados existe
    {
        let handles = SQLITE_CONNECTIONS.lock().unwrap();
        if !handles.contains_key(&p_db) {
            set_error(p_db, "no such connection".into());
            if pp_stmt != 0 {
                let p: MutPtr<u32> = MutPtr::from_bits(pp_stmt);
                env.mem.write(p, 0u32);
            }
            return SQLITE_ERROR;
        }
    }

    // Valida a query SQL tentando compilá-la nativamente no rusqlite
    let valid = {
        let handles = SQLITE_CONNECTIONS.lock().unwrap();
        let conn = handles.get(&p_db).unwrap();
        conn.prepare(&sql).is_ok()
    };

    if !valid {
        let handles = SQLITE_CONNECTIONS.lock().unwrap();
        let conn = handles.get(&p_db).unwrap();
        let err = conn.prepare(&sql).unwrap_err();
        let msg = format!("{}", err);
        log!("libsqlite3: prepare error: {}", msg);
        set_error(p_db, msg);
        if pp_stmt != 0 {
            let p: MutPtr<u32> = MutPtr::from_bits(pp_stmt);
            env.mem.write(p, 0u32);
        }
        return SQLITE_ERROR;
    }

    // Aloca o handle virtual e registra a declaração no emulador
    let stmt_handle = alloc_handle();
    SQLITE_STATEMENTS.lock().unwrap().insert(
        stmt_handle,
        StmtEntry {
            db_handle: p_db,
            sql,
            bindings: HashMap::new(),
            columns: Vec::new(),
            column_names: Vec::new(),
            column_name_ptrs: HashMap::new(),
            done: false,
        },
    );

    if pp_stmt != 0 {
        let p: MutPtr<u32> = MutPtr::from_bits(pp_stmt);
        env.mem.write(p, stmt_handle);
    }
    SQLITE_OK
}

// ---------- sqlite3_bind_int ----------
pub fn sqlite3_bind_int(_env: &mut Environment, stmt: u32, index: i32, value: i32) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry.bindings.insert(index as usize, BindValue::Int(value));
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_bind_int64 ----------
pub fn sqlite3_bind_int64(_env: &mut Environment, stmt: u32, index: i32, value: i64) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry
                .bindings
                .insert(index as usize, BindValue::Int64(value));
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_bind_double ----------
pub fn sqlite3_bind_double(_env: &mut Environment, stmt: u32, index: i32, value: f64) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry
                .bindings
                .insert(index as usize, BindValue::Double(value));
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_bind_text ----------
pub fn sqlite3_bind_text(
    env: &mut Environment,
    stmt: u32,
    index: i32,
    text_ptr: u32,
    n_bytes: i32,
    _destructor: u32,
) -> u32 {
    let text = if text_ptr == 0 {
        String::new()
    } else if n_bytes < 0 {
        read_cstring(env, text_ptr)
    } else {
        let mut bytes = Vec::with_capacity(n_bytes as usize);
        for i in 0..(n_bytes as u32) {
            let p: ConstPtr<u8> = ConstPtr::from_bits(text_ptr + i);
            bytes.push(env.mem.read(p));
        }
        String::from_utf8_lossy(&bytes).into_owned()
    };

    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry.bindings.insert(index as usize, BindValue::Text(text));
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_bind_blob ----------
pub fn sqlite3_bind_blob(
    env: &mut Environment,
    stmt: u32,
    index: i32,
    blob_ptr: u32,
    n_bytes: i32,
    _destructor: u32,
) -> u32 {
    let blob = if blob_ptr == 0 || n_bytes <= 0 {
        Vec::new()
    } else {
        let mut bytes = Vec::with_capacity(n_bytes as usize);
        for i in 0..(n_bytes as u32) {
            let p: ConstPtr<u8> = ConstPtr::from_bits(blob_ptr + i);
            bytes.push(env.mem.read(p));
        }
        bytes
    };

    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry.bindings.insert(index as usize, BindValue::Blob(blob));
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_bind_null ----------
pub fn sqlite3_bind_null(_env: &mut Environment, stmt: u32, index: i32) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry.bindings.insert(index as usize, BindValue::Null);
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_bind_blob ----------
pub fn sqlite3_bind_blob(
    env: &mut Environment,
    stmt: u32,
    index: i32,
    blob_ptr: u32,
    n_bytes: i32,
    _destructor: u32,
) -> u32 {
    let blob = if blob_ptr == 0 || n_bytes <= 0 {
        Vec::new()
    } else {
        let mut bytes = Vec::with_capacity(n_bytes as usize);
        for i in 0..(n_bytes as u32) {
            let p: ConstPtr<u8> = ConstPtr::from_bits(blob_ptr + i);
            bytes.push(env.mem.read(p));
        }
        bytes
    };

    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            entry.bindings.insert(index as usize, BindValue::Blob(blob));
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

// ---------- sqlite3_step ----------
pub fn sqlite3_step(env: &mut Environment, stmt: u32) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    let entry = match stmts.get_mut(&stmt) {
        Some(e) => e,
        None => return SQLITE_MISUSE,
    };

    if entry.done {
        return SQLITE_DONE;
    }

    let handles = SQLITE_CONNECTIONS.lock().unwrap();
    let conn = match handles.get(&entry.db_handle) {
        Some(c) => c,
        None => return SQLITE_ERROR,
    };

    // Prepara e executa a query nativamente no rusqlite
    let mut native_stmt = match conn.prepare(&entry.sql) {
        Ok(s) => s,
        Err(e) => {
            log!("libsqlite3: sqlite3_step prepare error: {}", e);
            return SQLITE_ERROR;
        }
    };

    // Mapeia e vincula todos os parâmetros dinâmicos (bindings) salvos
    for (idx, val) in &entry.bindings {
        let res = match val {
            BindValue::Int(i) => native_stmt.bind(( *idx, *i )),
            BindValue::Int64(i) => native_stmt.bind(( *idx, *i )),
            BindValue::Double(f) => native_stmt.bind(( *idx, *f )),
            BindValue::Text(s) => native_stmt.bind(( *idx, s.as_str() )),
            BindValue::Blob(b) => native_stmt.bind(( *idx, b.as_slice() )),
            BindValue::Null => native_stmt.bind(( *idx, rusqlite::types::Null )),
        };
        if res.is_err() {
            return SQLITE_ERROR;
        }
    }

    // Processa a leitura das colunas
    match native_stmt.query_row([], |row| {
        let mut cols = Vec::new();
        let count = row.as_ref().column_count();
        
        // Popula as colunas lazily para o jogo ler com sqlite3_column_*
        for i in 0..count {
            if let Ok(val) = row.get_ref(i) {
                let mapped = match val {
                    rusqlite::types::ValueRef::Null => ColumnValue::Null,
                    rusqlite::types::ValueRef::Integer(i) => ColumnValue::Int(i),
                    rusqlite::types::ValueRef::Real(f) => ColumnValue::Double(f),
                    rusqlite::types::ValueRef::Text(t) => {
                        let s = String::from_utf8_lossy(t).into_owned();
                        ColumnValue::Text(s)
                    }
                    rusqlite::types::ValueRef::Blob(b) => ColumnValue::Blob(b.to_vec()),
                };
                cols.push(mapped);
            }
        }
        Ok(cols)
    }) {
        Ok(columns) => {
            entry.columns = columns;
            // Se o jogo precisar ler nomes das colunas, populamos aqui
            if entry.column_names.is_empty() {
                entry.column_names = native_stmt.column_names().iter().map(|s| s.to_string()).collect();
            }
            SQLITE_ROW
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            entry.done = true;
            SQLITE_DONE
        }
        Err(e) => {
            log!("libsqlite3: sqlite3_step query execution error: {}", e);
            SQLITE_ERROR
        }
    }
}

// ---------- sqlite3_column_int ----------
pub fn sqlite3_column_int(_env: &mut Environment, stmt: u32, col: i32) -> i32 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt) {
        Some(entry) => {
            if let Some(ColumnValue::Int(val)) = entry.columns.get(col as usize) {
                *val as i32
            } else {
                0
            }
        }
        None => 0,
    }
}

// ---------- sqlite3_column_int64 ----------
pub fn sqlite3_column_int64(_env: &mut Environment, stmt: u32, col: i32) -> i64 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt) {
        Some(entry) => {
            if let Some(ColumnValue::Int(val)) = entry.columns.get(col as usize) {
                *val
            } else {
                0
            }
        }
        None => 0,
    }
}

// ---------- sqlite3_column_text ----------
pub fn sqlite3_column_text(env: &mut Environment, stmt: u32, col: i32) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get_mut(&stmt) {
        Some(entry) => {
            if let Some(ColumnValue::Text(val)) = entry.columns.get(col as usize) {
                // Aloca dinamicamente na memória convidada (Guest mem) para o iOS ler a string
                if let Some(&ptr) = entry.column_name_ptrs.get(&col) {
                    ptr
                } else {
                    let bytes = val.as_bytes();
                    let len = bytes.len() as u32;
                    // Força alocação segura na Heap virtual do emulador
                    if let Ok(allocated_ptr) = env.heap.alloc(len + 1) {
                        for (i, &b) in bytes.iter().enumerate() {
                            let p: MutPtr<u8> = MutPtr::from_bits(allocated_ptr + i as u32);
                            env.mem.write(p, b);
                        }
                        let p_tail: MutPtr<u8> = MutPtr::from_bits(allocated_ptr + len);
                        env.mem.write(p_tail, 0u8); // Nul terminator \0

                        entry.column_name_ptrs.insert(col, allocated_ptr);
                        allocated_ptr
                    } else {
                        0
                    }
                }
            } else {
                0
            }
        }
        None => 0,
    }
}

// ---------- sqlite3_finalize ----------
pub fn sqlite3_finalize(env: &mut Environment, stmt: u32) -> u32 {
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    if let Some(entry) = stmts.remove(&stmt) {
        // Desaloca ponteiros de strings temporários criados para esta query
        for (_col, ptr) in entry.column_name_ptrs {
            let _ = env.heap.free(ptr);
        }
        SQLITE_OK
    } else {
        SQLITE_ERROR
    }
}

// ---------- sqlite3_column_count ----------
pub fn sqlite3_column_count(_env: &mut Environment, stmt_handle: u32) -> i32 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => entry.columns.len() as i32,
        None => 0,
    }
}

// ---------- sqlite3_column_int ----------
pub fn sqlite3_column_int(_env: &mut Environment, stmt_handle: u32, col: i32) -> i32 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => match entry.columns.get(col as usize) {
            Some(ColumnValue::Int(v)) => *v as i32,
            Some(ColumnValue::Double(v)) => *v as i32,
            Some(ColumnValue::Text(v)) => v.parse::<i32>().unwrap_or(0),
            _ => 0,
        },
        None => 0,
    }
}

// ---------- sqlite3_column_int64 ----------
pub fn sqlite3_column_int64(_env: &mut Environment, stmt_handle: u32, col: i32) -> i64 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => match entry.columns.get(col as usize) {
            Some(ColumnValue::Int(v)) => *v,
            Some(ColumnValue::Double(v)) => *v as i64,
            Some(ColumnValue::Text(v)) => v.parse::<i64>().unwrap_or(0),
            _ => 0,
        },
        None => 0,
    }
}

// ---------- sqlite3_column_double ----------
pub fn sqlite3_column_double(_env: &mut Environment, stmt_handle: u32, col: i32) -> f64 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => match entry.columns.get(col as usize) {
            Some(ColumnValue::Double(v)) => *v,
            Some(ColumnValue::Int(v)) => *v as f64,
            Some(ColumnValue::Text(v)) => v.parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        },
        None => 0.0,
    }
}

// ---------- sqlite3_column_text ----------
// Returns a pointer to a static/leaked string in guest memory.
// This is a simplification - real SQLite returns pointer valid until next step/finalize.
pub fn sqlite3_column_text(env: &mut Environment, stmt_handle: u32, col: i32) -> u32 {
    let text = {
        let stmts = SQLITE_STATEMENTS.lock().unwrap();
        match stmts.get(&stmt_handle) {
            Some(entry) => match entry.columns.get(col as usize) {
                Some(ColumnValue::Text(v)) => v.clone(),
                Some(ColumnValue::Int(v)) => format!("{}", v),
                Some(ColumnValue::Double(v)) => format!("{}", v),
                _ => return 0,
            },
            None => return 0,
        }
    };

    // Allocate in guest memory
    let len = text.len() + 1; // +1 for null terminator
    let guest_ptr = env.mem.alloc(len.try_into().unwrap());
    let base = guest_ptr.to_bits();
    for (i, b) in text.as_bytes().iter().enumerate() {
        let p: MutPtr<u8> = MutPtr::from_bits(base + i as u32);
        env.mem.write(p, *b);
    }
    let p: MutPtr<u8> = MutPtr::from_bits(base + text.len() as u32);
    env.mem.write(p, 0u8);
    base
}

// ---------- sqlite3_column_bytes ----------
pub fn sqlite3_column_bytes(_env: &mut Environment, stmt_handle: u32, col: i32) -> i32 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => match entry.columns.get(col as usize) {
            Some(ColumnValue::Text(v)) => v.len() as i32,
            Some(ColumnValue::Blob(v)) => v.len() as i32,
            Some(ColumnValue::Int(_)) => 4,
            Some(ColumnValue::Double(_)) => 8,
            _ => 0,
        },
        None => 0,
    }
}

// ---------- sqlite3_column_type ----------
pub fn sqlite3_column_type(_env: &mut Environment, stmt_handle: u32, col: i32) -> i32 {
    const SQLITE_INTEGER: i32 = 1;
    const SQLITE_FLOAT: i32 = 2;
    const SQLITE_TEXT: i32 = 3;
    const SQLITE_BLOB: i32 = 4;
    const SQLITE_NULL: i32 = 5;

    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => match entry.columns.get(col as usize) {
            Some(ColumnValue::Int(_)) => SQLITE_INTEGER,
            Some(ColumnValue::Double(_)) => SQLITE_FLOAT,
            Some(ColumnValue::Text(_)) => SQLITE_TEXT,
            Some(ColumnValue::Blob(_)) => SQLITE_BLOB,
            Some(ColumnValue::Null) | None => SQLITE_NULL,
        },
        None => 5, // SQLITE_NULL
    }
}

// ---------- sqlite3_column_name ----------
//
// Returns a pointer to a NUL-terminated UTF-8 string naming the `col`-th
// result column of the prepared statement. Per SQLite's documentation
// (<https://www.sqlite.org/c3ref/column_name.html>):
//
//   "The returned string pointer is valid until either the prepared
//   statement is destroyed by sqlite3_finalize() or until the statement is
//   automatically reprepared by the first call to sqlite3_step() for a
//   particular run or until the next call to sqlite3_column_name() or
//   sqlite3_column_name16() on the same column."
//
// We satisfy this by keeping one guest-side allocation per (stmt, col), so
// repeated calls return the same pointer.
pub fn sqlite3_column_name(env: &mut Environment, stmt_handle: u32, col: i32) -> u32 {
    // Look up name and any existing guest pointer under the lock.
    let (name, cached) = {
        let stmts = SQLITE_STATEMENTS.lock().unwrap();
        match stmts.get(&stmt_handle) {
            Some(entry) => {
                let name = entry
                    .column_names
                    .get(col as usize)
                    .cloned()
                    .unwrap_or_default();
                (name, entry.column_name_ptrs.get(&col).copied())
            }
            None => return 0,
        }
    };

    if let Some(p) = cached {
        // Cached entry from a previous call. The SQLite contract
        // guarantees the pointer is stable until the next step/finalize.
        return p;
    }

    let bytes = name.as_bytes();
    let len = bytes.len() + 1;
    let guest_ptr = env.mem.alloc(len.try_into().unwrap());
    let base = guest_ptr.to_bits();
    for (i, b) in bytes.iter().enumerate() {
        let p: MutPtr<u8> = MutPtr::from_bits(base + i as u32);
        env.mem.write(p, *b);
    }
    let p: MutPtr<u8> = MutPtr::from_bits(base + bytes.len() as u32);
    env.mem.write(p, 0u8);

    // Cache the guest pointer for this column.
    let mut stmts = SQLITE_STATEMENTS.lock().unwrap();
    if let Some(entry) = stmts.get_mut(&stmt_handle) {
        entry.column_name_ptrs.insert(col, base);
    }
    base
}

// ---------- sqlite3_data_count ----------
//
// Returns the number of columns in the current result row of a prepared
// statement. Per <https://www.sqlite.org/c3ref/data_count.html>, this is
// the run-time analogue of `sqlite3_column_count`: it returns 0 after
// `SQLITE_DONE`, and the same value as `sqlite3_column_count` after
// `SQLITE_ROW`.
pub fn sqlite3_data_count(_env: &mut Environment, stmt_handle: u32) -> i32 {
    let stmts = SQLITE_STATEMENTS.lock().unwrap();
    match stmts.get(&stmt_handle) {
        Some(entry) => entry.columns.len() as i32,
        None => 0,
    }
}

// ---------- sqlite3_column_blob ----------
//
// Returns a pointer to BLOB column data. SQLite docs guarantee the pointer
// is valid until the next step/reset/finalize for that column, so we
// allocate fresh guest memory each call and let the guest reuse it for
// the same row.
pub fn sqlite3_column_blob(env: &mut Environment, stmt_handle: u32, col: i32) -> u32 {
    let blob = {
        let stmts = SQLITE_STATEMENTS.lock().unwrap();
        match stmts.get(&stmt_handle) {
            Some(entry) => match entry.columns.get(col as usize) {
                Some(ColumnValue::Blob(v)) => v.clone(),
                Some(ColumnValue::Text(v)) => v.as_bytes().to_vec(),
                _ => return 0,
            },
            None => return 0,
        }
    };
    if blob.is_empty() {
        // Real SQLite returns a non-NULL pointer to a zero-length blob, but
        // returning 0 here matches what `sqlite3_column_text` does for empty
        // text — the corresponding `sqlite3_column_bytes` will report 0 too.
        return 0;
    }
    let guest_ptr = env.mem.alloc(blob.len().try_into().unwrap());
    let base = guest_ptr.to_bits();
    for (i, b) in blob.iter().enumerate() {
        let p: MutPtr<u8> = MutPtr::from_bits(base + i as u32);
        env.mem.write(p, *b);
    }
    base
}

// ---------- sqlite3_errmsg ----------
// Returns pointer to error string in guest memory
pub fn sqlite3_errmsg(env: &mut Environment, p_db: u32) -> u32 {
    let msg = {
        let errors = LAST_ERROR.lock().unwrap();
        errors
            .get(&p_db)
            .cloned()
            .unwrap_or_else(|| "not an error".to_string())
    };

    let len = msg.len() + 1;
    let guest_ptr = env.mem.alloc(len.try_into().unwrap());
    let base = guest_ptr.to_bits();
    for (i, b) in msg.as_bytes().iter().enumerate() {
        let p: MutPtr<u8> = MutPtr::from_bits(base + i as u32);
        env.mem.write(p, *b);
    }
    let p: MutPtr<u8> = MutPtr::from_bits(base + msg.len() as u32);
    env.mem.write(p, 0u8);
    base
}

// ---------- sqlite3_changes ----------
pub fn sqlite3_changes(_env: &mut Environment, p_db: u32) -> i32 {
    let handles = SQLITE_CONNECTIONS.lock().unwrap();
    match handles.get(&p_db) {
        Some(_conn) => 0, // rusqlite doesn't easily expose this without a statement
        None => 0,
    }
}

// ---------- sqlite3_last_insert_rowid ----------
pub fn sqlite3_last_insert_rowid(_env: &mut Environment, p_db: u32) -> i64 {
    let handles = SQLITE_CONNECTIONS.lock().unwrap();
    match handles.get(&p_db) {
        Some(conn) => conn.last_insert_rowid(),
        None => 0,
    }
}

// ---------- sqlite3_free ----------
pub fn sqlite3_free(env: &mut Environment, ptr: u32) {
    if ptr == 0 {
        return;
    }
    let mem_ptr = crate::mem::MutVoidPtr::from_bits(ptr);
    env.mem.free(mem_ptr);
}

// ---------- sqlite3_busy_timeout ----------
pub fn sqlite3_busy_timeout(_env: &mut Environment, _p_db: u32, _ms: i32) -> u32 {
    SQLITE_OK
}

// ---------- sqlite3_errcode ----------
pub fn sqlite3_errcode(_env: &mut Environment, _p_db: u32) -> u32 {
    SQLITE_OK
}

pub fn sqlite3_libversion(env: &mut Environment) -> u32 {
    let text = "3.7.0\0";

    let ptr = env.mem.alloc(text.len().try_into().unwrap());
    let base = ptr.to_bits();

    for (i, b) in text.as_bytes().iter().enumerate() {
        let p: MutPtr<u8> = MutPtr::from_bits(base + i as u32);
        env.mem.write(p, *b);
    }

    base
}

pub fn sqlite3_initialize(_env: &mut Environment) -> u32 {
    SQLITE_OK
}

pub fn sqlite3_threadsafe(_env: &mut Environment) -> i32 {
    1
}

// Export all functions
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sqlite3_open(_, _)),
    export_c_func!(sqlite3_open_v2(_, _, _, _)),
    export_c_func!(sqlite3_close(_)),
    export_c_func!(sqlite3_close_v2(_)),
    export_c_func!(sqlite3_exec(_, _, _, _, _)),
    export_c_func!(sqlite3_prepare(_, _, _, _, _)),
    export_c_func!(sqlite3_prepare_v2(_, _, _, _, _)),
    export_c_func!(sqlite3_bind_int(_, _, _)),
    export_c_func!(sqlite3_bind_int64(_, _, _)),
    export_c_func!(sqlite3_bind_double(_, _, _)),
    export_c_func!(sqlite3_bind_text(_, _, _, _, _)),
    export_c_func!(sqlite3_bind_blob(_, _, _, _, _)),
    export_c_func!(sqlite3_bind_null(_, _)),
    export_c_func!(sqlite3_step(_)),
    export_c_func!(sqlite3_reset(_)),
    export_c_func!(sqlite3_clear_bindings(_)),
    export_c_func!(sqlite3_finalize(_)),
    export_c_func!(sqlite3_column_count(_)),
    export_c_func!(sqlite3_column_int(_, _)),
    export_c_func!(sqlite3_column_int64(_, _)),
    export_c_func!(sqlite3_column_double(_, _)),
    export_c_func!(sqlite3_column_text(_, _)),
    export_c_func!(sqlite3_column_bytes(_, _)),
    export_c_func!(sqlite3_column_type(_, _)),
    export_c_func!(sqlite3_column_name(_, _)),
    export_c_func!(sqlite3_data_count(_)),
    export_c_func!(sqlite3_column_blob(_, _)),
    export_c_func!(sqlite3_errmsg(_)),
    export_c_func!(sqlite3_changes(_)),
    export_c_func!(sqlite3_last_insert_rowid(_)),
    export_c_func!(sqlite3_free(_)),
    export_c_func!(sqlite3_busy_timeout(_, _)),
    export_c_func!(sqlite3_errcode(_)),
    export_c_func!(sqlite3_libversion()),
    export_c_func!(sqlite3_initialize()),
    export_c_func!(sqlite3_threadsafe()),
];

pub const DYLIB: HostDylib = HostDylib {
    path: "/usr/lib/libsqlite3.dylib",
    aliases: &["/usr/lib/libsqlite3.0.dylib", "sqlite3", "libsqlite3", "libsqlite3.dylib", "sqlite3.dylib"],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};
