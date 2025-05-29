pub mod actions {
    pub mod results {
        pub mod api {
            pub mod v1 {
                include!(concat!(
                    env!("OUT_DIR"),
                    "/github.actions.results.api.v1.rs"
                ));
            }
        }
        pub mod entities {
            pub mod v1 {
                include!(concat!(
                    env!("OUT_DIR"),
                    "/github.actions.results.entities.v1.rs"
                ));
            }
        }
    }
}
