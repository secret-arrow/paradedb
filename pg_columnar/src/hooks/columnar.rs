use pgrx::*;
use std::ffi::{c_char, CStr, CString};

static COLUMNAR_HANDLER: &str = "mem";

pub struct ColumnarStmt;

impl ColumnarStmt {
    pub unsafe fn planned_is_columnar(ps: *mut pg_sys::PlannedStmt) -> Result<bool, String> {
        let rtable = (*ps).rtable;
        if rtable.is_null() {
            return Err("rtable is null".to_string());
        }

        let columnar_handler_oid = Self::columnar_handler_oid()?;

        let elements = (*rtable).elements;
        let mut using_noncol: bool = false;
        let mut using_col: bool = false;

        for i in 0..(*rtable).length {
            let rte = (*elements.offset(i as isize)).ptr_value as *mut pg_sys::RangeTblEntry;
            if (*rte).rtekind != pg_sys::RTEKind_RTE_RELATION {
                continue;
            }
            let relation = pg_sys::RelationIdGetRelation((*rte).relid);
            let pg_relation = PgRelation::from_pg_owned(relation);
            if !pg_relation.is_table() {
                continue;
            }

            let relation_handler_oid = (*relation).rd_amhandler;

            // If any table uses the Table AM handler, then return true.
            // TODO: if we support more operations, this will be more complex.
            //       for example, if to support joins, some of the nodes will use
            //       table AM for the nodes while others won't. In this case,
            //       we'll have to process in postgres plan for part of it and
            //       datafusion for the other part. For now, we'll simply
            //       fail if we encounter an unsupported node, so this won't happen.
            if relation_handler_oid == columnar_handler_oid {
                using_col = true;
            } else {
                using_noncol = true;
            }
        }

        if using_col && using_noncol {
            return Err("Mixing table types in a single query is not supported yet".to_string());
        }

        Ok(using_col)
    }

    pub unsafe fn copy_is_columnar(copy_stmt: *mut pg_sys::CopyStmt) -> Result<bool, String> {
        let columnar_handler_oid = Self::columnar_handler_oid()?;
        let relation_handler_oid = Self::relation_handler_oid((*copy_stmt).relation)?;

        Ok(relation_handler_oid == columnar_handler_oid)
    }

    pub unsafe fn relation_is_columnar(
        relation: *mut pg_sys::RelationData,
    ) -> Result<bool, String> {
        let columnar_handler_oid = Self::columnar_handler_oid()?;
        let relation_handler_oid = (*relation).rd_amhandler;

        Ok(relation_handler_oid == columnar_handler_oid)
    }

    unsafe fn columnar_handler_oid() -> Result<pg_sys::Oid, String> {
        let columnar_handler_str = CString::new(COLUMNAR_HANDLER).unwrap();
        let columnar_handler_ptr = columnar_handler_str.as_ptr() as *const c_char;

        let columnar_oid = pg_sys::get_am_oid(columnar_handler_ptr, true);
        if columnar_oid == pg_sys::InvalidOid {
            return Err("Columnar handler not found".to_string());
        }

        let heap_tuple_data = pg_sys::SearchSysCache1(
            pg_sys::SysCacheIdentifier_AMOID as i32,
            pg_sys::Datum::from(columnar_oid),
        );
        let catalog = pg_sys::heap_tuple_get_struct::<pg_sys::FormData_pg_am>(heap_tuple_data);
        pg_sys::ReleaseSysCache(heap_tuple_data);

        Ok((*catalog).amhandler)
    }

    unsafe fn relation_handler_oid(relation: *mut pg_sys::RangeVar) -> Result<pg_sys::Oid, String> {
        let relation_name = CStr::from_ptr((*relation).relname).to_str().unwrap();
        let relation_data = PgRelation::open_with_name(relation_name)?.as_ptr();

        Ok((*relation_data).rd_amhandler)
    }
}