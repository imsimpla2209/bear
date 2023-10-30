use std::fs::File;
use std::io::Write;

pub fn export_ts(fname: &str, found_types: Vec<Result<String, ts_rs::ExportError>>) {
    let mut f = File::create(fname).unwrap();

    let re_import = regex::RegexBuilder::new(r"^import type .*;$\n")
        .multi_line(true)
        .build()
        .unwrap();
    let re_comment = regex::RegexBuilder::new(r"// This file .*$\n")
        .multi_line(true)
        .build()
        .unwrap();

    for it in found_types {
        let orig = it.unwrap();
        let replaced = re_import.replace_all(&orig, "");
        let tstr = re_comment.replace_all(
            &replaced,
            "");
        f.write_all(tstr.as_bytes()).unwrap();
    }
}

#[macro_export]
macro_rules! register_types {
    (  $($x:ty),+  ) => {
        pub fn found_types() -> Vec<Result<String, ts_rs::ExportError>> {
            let mut res = Vec::new();
            $(
                res.push(<$x>::export_to_string());
            )+
            return res
        }

        pub struct SchemasMod;

        impl Modify for SchemasMod {
            fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
                openapi.components = Some(
                    utoipa::openapi::ComponentsBuilder::new()
                    $(
                        .schema_from::<$x>()
                    )+
                        .build()
                    )
            }
        }
    }
}
