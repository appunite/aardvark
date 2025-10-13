use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use capnp::message::Builder;
use capnp::serialize;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct Lockfile {
    info: Info,
    packages: BTreeMap<String, Package>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Info {
    abi_version: String,
    arch: String,
    platform: String,
    python: String,
    version: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Package {
    #[serde(default)]
    depends: Vec<String>,
    file_name: String,
    #[serde(default)]
    imports: Vec<String>,
    install_dir: String,
    name: String,
    package_type: String,
    sha256: String,
    #[serde(default)]
    unvendored_tests: bool,
    version: String,
}

mod schema {
    use capnp::private::layout::{ListBuilder, PointerBuilder, StructBuilder, StructSize};
    use capnp::text_list;
    use capnp::traits::FromPointerBuilder;
    use capnp::Result;

    const PACKAGE_METADATA_SIZE: StructSize = StructSize {
        data: 0,
        pointers: 2,
    };
    const INFO_SIZE: StructSize = StructSize {
        data: 0,
        pointers: 5,
    };
    const PACKAGE_SIZE: StructSize = StructSize {
        data: 1,
        pointers: 9,
    };

    pub struct PackageMetadataBuilder<'a> {
        builder: StructBuilder<'a>,
    }

    impl<'a> FromPointerBuilder<'a> for PackageMetadataBuilder<'a> {
        fn init_pointer(mut pointer: PointerBuilder<'a>, _len: u32) -> Self {
            if !pointer.is_null() {
                pointer.clear();
            }
            let builder = pointer.init_struct(PACKAGE_METADATA_SIZE);
            Self { builder }
        }

        fn get_from_pointer(
            pointer: PointerBuilder<'a>,
            default: Option<&'a [capnp::Word]>,
        ) -> Result<Self> {
            let builder = pointer.get_struct(PACKAGE_METADATA_SIZE, default)?;
            Ok(Self { builder })
        }
    }

    impl<'a> PackageMetadataBuilder<'a> {
        pub fn init_info(&mut self) -> InfoBuilder<'_> {
            let builder = self
                .builder
                .reborrow()
                .get_pointer_field(0)
                .init_struct(INFO_SIZE);
            InfoBuilder { builder }
        }

        pub fn init_packages(&mut self, len: u32) -> PackagesBuilder<'_> {
            let list = self
                .builder
                .reborrow()
                .get_pointer_field(1)
                .init_struct_list(len, PACKAGE_SIZE);
            PackagesBuilder { builder: list }
        }
    }

    pub struct InfoBuilder<'a> {
        builder: StructBuilder<'a>,
    }

    impl<'a> InfoBuilder<'a> {
        pub fn set_version(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(0)
                .set_text(value.into());
        }
        pub fn set_python(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(1)
                .set_text(value.into());
        }
        pub fn set_abi_version(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(2)
                .set_text(value.into());
        }
        pub fn set_arch(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(3)
                .set_text(value.into());
        }
        pub fn set_platform(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(4)
                .set_text(value.into());
        }
    }

    pub struct PackagesBuilder<'a> {
        builder: ListBuilder<'a>,
    }

    impl<'a> PackagesBuilder<'a> {
        pub fn get(&mut self, index: u32) -> PackageBuilder<'_> {
            PackageBuilder {
                builder: self.builder.reborrow().get_struct_element(index),
            }
        }
    }

    pub struct PackageBuilder<'a> {
        builder: StructBuilder<'a>,
    }

    impl<'a> PackageBuilder<'a> {
        pub fn set_canonical_name(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(0)
                .set_text(value.into());
        }
        pub fn set_name(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(1)
                .set_text(value.into());
        }
        pub fn set_file_name(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(2)
                .set_text(value.into());
        }
        pub fn set_install_dir(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(3)
                .set_text(value.into());
        }
        pub fn set_package_type(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(4)
                .set_text(value.into());
        }
        pub fn set_sha256(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(5)
                .set_text(value.into());
        }
        pub fn set_version(&mut self, value: &str) {
            self.builder
                .reborrow()
                .get_pointer_field(6)
                .set_text(value.into());
        }
        pub fn set_unvendored_tests(&mut self, value: bool) {
            self.builder.set_bool_field(0, value);
        }
        pub fn init_imports(&mut self, len: u32) -> text_list::Builder<'_> {
            text_list::Builder::init_pointer(self.builder.reborrow().get_pointer_field(7), len)
        }
        pub fn init_depends(&mut self, len: u32) -> text_list::Builder<'_> {
            text_list::Builder::init_pointer(self.builder.reborrow().get_pointer_field(8), len)
        }
    }
}

struct MetadataCache {
    json: Arc<str>,
    capnp: Arc<[u8]>,
}

static METADATA_CACHE: OnceLock<MetadataCache> = OnceLock::new();

/// Returns the Pyodide package metadata encoded as Cap'n Proto bytes.
pub fn package_metadata_capnp() -> Arc<[u8]> {
    get_cache().capnp.clone()
}

/// Returns the Pyodide package metadata as canonical JSON (derived from the Cap'n Proto model).
pub fn package_metadata_json() -> Arc<str> {
    get_cache().json.clone()
}

fn get_cache() -> &'static MetadataCache {
    METADATA_CACHE.get_or_init(|| {
        let build = build_metadata();
        MetadataCache {
            json: Arc::<str>::from(build.json_text),
            capnp: Arc::<[u8]>::from(build.capnp_bytes.into_boxed_slice()),
        }
    })
}

struct BuildResult {
    json_text: String,
    capnp_bytes: Vec<u8>,
}

fn build_metadata() -> BuildResult {
    let raw_json = crate::assets::lockfile_json_raw();
    let lockfile: Lockfile =
        serde_json::from_str(raw_json).expect("pyodide lockfile JSON should be valid");

    let mut message = Builder::new_default();
    {
        let mut root = message.init_root::<schema::PackageMetadataBuilder>();

        {
            let mut info = root.init_info();
            info.set_version(&lockfile.info.version);
            info.set_python(&lockfile.info.python);
            info.set_abi_version(&lockfile.info.abi_version);
            info.set_arch(&lockfile.info.arch);
            info.set_platform(&lockfile.info.platform);
        }

        let mut packages = root.init_packages(lockfile.packages.len() as u32);
        for (idx, (canonical, pkg)) in lockfile.packages.iter().enumerate() {
            let mut entry = packages.get(idx as u32);
            entry.set_canonical_name(canonical);
            entry.set_name(&pkg.name);
            entry.set_file_name(&pkg.file_name);
            entry.set_install_dir(&pkg.install_dir);
            entry.set_package_type(&pkg.package_type);
            entry.set_sha256(&pkg.sha256);
            entry.set_version(&pkg.version);
            entry.set_unvendored_tests(pkg.unvendored_tests);

            let mut imports = entry.init_imports(pkg.imports.len() as u32);
            for (i, import) in pkg.imports.iter().enumerate() {
                imports.set(i as u32, import.as_str().into());
            }

            let mut depends = entry.init_depends(pkg.depends.len() as u32);
            for (i, dep) in pkg.depends.iter().enumerate() {
                depends.set(i as u32, dep.as_str().into());
            }
        }
    }

    let mut capnp_bytes = Vec::new();
    serialize::write_message(&mut capnp_bytes, &message)
        .expect("serialize package metadata to capnp");

    let json_text = serde_json::to_string(&lockfile).expect("serialize canonical lockfile json");

    BuildResult {
        json_text,
        capnp_bytes,
    }
}
