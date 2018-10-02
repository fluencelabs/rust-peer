//! Module wrapper for Llamadb.
//!
//! It provides public methods for work with Llamadb and for `allocation` and
//! `deallocation` memory from a WASM host environment. Also contains functions
//! for dereference string arguments for passing theirs into WASM functions.

#![feature(allocator_api)]
#![allow(dead_code)]

#[macro_use]
extern crate lazy_static;
extern crate llamadb;

use std::ptr::NonNull;
use std::alloc::{Alloc, Global, Layout};
use std::mem;
use std::error::Error;
use std::sync::Mutex;
use llamadb::tempdb::TempDb;
use llamadb::tempdb::ExecuteStatementResponse;

/// Result for all possible Error types.
type GenResult<T> = Result<T, Box<Error>>;

//
// Public functions for work with Llamadb.
//

/// Execute sql and returns result as string in the memory.
///
/// 1. Takes a pointer and length for a SQL string in memory, makes from them
///    Rust string.
/// 2. Processes the query for specified SQL string
/// 3. Returns a pointer to a result as a string in the memory.
#[no_mangle]
pub unsafe fn do_query(ptr: *mut u8, len: usize) -> i32 {
    let sql_str = deref_str(ptr, len);
    let db_response = match run_query(&sql_str) {
        Ok(response) => { response },
        Err(err_msg) => { err_msg.description().to_string() },
    };
    put_to_mem(db_response)
}

//
// Public functions for memory management
//

/// Allocates memory area of specified size and returns address of the first
/// byte in the allocated memory area.
#[no_mangle]
pub unsafe fn alloc(size: usize) -> NonNull<u8> {
    allocate(size)
        .expect(format!("[Error] Allocation of {} bytes failed.", size).as_str())
}

unsafe fn allocate(size: usize) -> GenResult<NonNull<u8>> {
    let layout: Layout = Layout::from_size_align(size, mem::align_of::<u8>())?;
    Global.alloc(layout).map_err(Into::into)
}

/// Deallocates memory area with first byte address = `ptr` and size = `size`.
#[no_mangle]
pub unsafe fn dealloc(ptr: NonNull<u8>, size: usize) -> () {
    deallocate(ptr, size)
        .expect(format!("[Error] Deallocate failed for prt={:?} size={}.", ptr, size).as_str())
}

unsafe fn deallocate(ptr: NonNull<u8>, size: usize) -> GenResult<()> {
    let layout = Layout::from_size_align(size, mem::align_of::<u8>())?;
    Ok(Global.dealloc(ptr, layout))
}

//
// Private functions with working with Strings and Memory.
//

/// Builds Rust string from the pointer and length.
unsafe fn deref_str(ptr: *mut u8, len: usize) -> String {
    String::from_raw_parts(ptr, len, len)
}

/// Acquires lock, does query, releases lock, returns query result
fn run_query(sql_query: &str) -> GenResult<String> {
    let statement = llamadb::sqlsyntax::parse_statement(sql_query);
    let mut db = DATABASE.lock()?;
    let result = db.execute_statement(statement)
        .map(statement_to_csv)
        .map_err(Into::into);
    result
}

/// Converts query result to CSV String.
fn statement_to_csv<'a>(_statement: ExecuteStatementResponse<'a>) -> String {
    // todo implement
    unimplemented!()
}

/// todo docs
fn put_to_mem(str: String) -> i32 {
    // puts string to memory, create wat pointer, return adress of first byte
    // todo implement
    str.len() as i32
}

/// Creates a public static reference to initialized LlamsDb instance.
lazy_static! {

    static ref DATABASE: Mutex<TempDb> = Mutex::new(TempDb::new());

}