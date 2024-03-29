use anyhow::Result;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::Path;

mod convert;

mod types;
use types::*;

pub struct GeneratorConfig {
    pub address_size: AddressSize,
    pub ignore: Vec<String>,
    pub arch_crate: String,
}

impl GeneratorConfig {
    pub fn address_size(mut self, size: AddressSize) -> Self {
        self.address_size = size;
        self
    }

    pub fn ignore<I>(mut self, ignore: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.ignore = ignore.into_iter().map(|s| s.into()).collect();
        self
    }

    pub fn arch_crate<S>(mut self, path: S) -> Self
    where
        S: Into<String>,
    {
        self.arch_crate = path.into();
        self
    }
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        GeneratorConfig {
            address_size: AddressSize::U32,
            ignore: Vec::new(),
            arch_crate: "crate::arch".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressSize {
    U32,
    U64,
}

impl AddressSize {
    pub fn type_name(&self) -> &'static str {
        match self {
            AddressSize::U32 => "u32",
            AddressSize::U64 => "u64",
        }
    }
}

/*
mod register;
pub use crate::register::{...};

pub mod stm32f1;
pub use stm32f1::stm32f103::*;

mod stm32f1 {
    pub mod peripherals;
    pub(crate) mod instances;
    pub mod stm32f103;
}

*/

pub fn generate(xml: &str, output_dir: impl AsRef<Path>, config: &GeneratorConfig) -> Result<()> {
    let device = svd_parser::parse(xml)?;

    let soc_name = device.name.to_ascii_lowercase();
    let soc_dir = output_dir.as_ref().join(soc_name);
    if soc_dir.is_dir() {
        fs::remove_dir_all(&soc_dir)?;
    }
    fs::create_dir(&soc_dir)?;

    let peripherals_dir = soc_dir.join("peripherals");
    fs::create_dir(&peripherals_dir)?;

    let instances_dir = soc_dir.join("instances");
    fs::create_dir(&instances_dir)?;

    let mut mod_rs = fs::File::create(soc_dir.join("mod.rs"))?;
    let mut peripherals_mod_rs = fs::File::create(peripherals_dir.join("mod.rs"))?;
    let mut instances_mod_rs = fs::File::create(instances_dir.join("mod.rs"))?;
    let mut metadata_rs = fs::File::create(soc_dir.join("metadata.rs"))?;

    writeln!(
        mod_rs,
        "
/// Peripherals shared by multiple devices
pub mod peripherals;

/// Peripheral instances shared by multiple devices
pub(crate) mod instances;

/// Metadata
pub mod metadata;
"
    )?;

    let device = crate::convert::convert(&device);

    let mut peripheral_modules = Vec::new();
    let mut instance_modules = Vec::new();
    let mut instance_names = Vec::new();

    for peripheral in &device.peripherals {
        if config.ignore.iter().any(|v| v == &peripheral.name) {
            continue;
        }

        writeln!(peripherals_mod_rs, "pub mod {};", peripheral.module_name)?;

        //generate_peripheral(&peripherals_dir, &instances_dir, &device, peripheral)?;
        let file_name = format!("{}.rs", peripheral.module_name);
        let mut peripheral_rs = fs::File::create(peripherals_dir.join(&file_name))?;

        write_peripheral(&mut peripheral_rs, peripheral, config)?;

        peripheral_modules.push(peripheral.module_name.clone());
    }

    for instance in &device.instances {
        if config.ignore.iter().any(|v| v == &instance.name) {
            continue;
        }

        writeln!(instances_mod_rs, "pub mod {};", instance.module_name)?;
        writeln!(mod_rs, "pub use self::instances::{};", instance.module_name)?;

        let file_name = format!("{}.rs", instance.module_name);
        let mut instance_rs = fs::File::create(instances_dir.join(&file_name))?;

        write_peripheral_instance(&mut instance_rs, instance, config)?;

        instance_modules.push(instance.module_name.clone());
        instance_names.push(instance.name.clone());
    }

    writeln!(metadata_rs, "pub const PERIPHERAL_MODULES: &[&str] = &[")?;
    for name in &peripheral_modules {
        writeln!(metadata_rs, "    \"{}\",", name)?;
    }
    writeln!(metadata_rs, "];\n")?;
    writeln!(metadata_rs, "pub const INSTANCE_MODULES: &[&str] = &[")?;
    for name in &instance_modules {
        writeln!(metadata_rs, "    \"{}\",", name)?;
    }
    writeln!(metadata_rs, "];\n")?;
    writeln!(metadata_rs, "pub const INSTANCE_NAMES: &[&str] = &[")?;
    for name in &instance_names {
        writeln!(metadata_rs, "    \"{}\",", name)?;
    }
    writeln!(metadata_rs, "];")?;

    Ok(())
}

fn write_peripheral(file: &mut fs::File, peripheral: &ModelPeripheral, config: &GeneratorConfig) -> Result<()> {
    writeln!(file, "#![allow(non_snake_case, non_upper_case_globals)]")?;
    writeln!(file, "#![allow(non_camel_case_types)]")?;
    writeln!(file, "{}", build_doc_comment("//!", &peripheral.description))?;

    let mut register_modules = Vec::new();
    let mut register_block = Vec::new();
    let mut reset_values = Vec::new();
    let mut access_types = BTreeSet::new();
    let mut register_types = Vec::new();

    for reg_info in &peripheral.registers {
        let access_type_name = reg_info.properties.access_type_name();
        access_types.insert(access_type_name);

        let size_type_name = reg_info.properties.size_type_name();

        register_types.push(reg_info.name.clone());

        // Register module
        let mut code = String::new();
        if let Some(description) = reg_info.description.as_ref() {
            let doc = build_doc_comment("///", description);
            code += &doc;
        }
        writeln!(code, "pub mod {} {{", reg_info.name)?;
        let mut field_strings = Vec::new();
        for field in &reg_info.fields {
            field_strings.push(field.generate_code())
        }
        code += &field_strings.join("\n");
        writeln!(code, "}}")?;
        register_modules.push(code);

        // RegisterBlock entry
        let mut s = String::new();
        if let Some(description) = reg_info.description.as_ref() {
            let doc = build_doc_comment("    ///", description);
            s += &doc;
        }
        writeln!(
            s,
            "    pub {}: {}<{}>,",
            reg_info.name, access_type_name, size_type_name
        )?;
        register_block.push(s);

        // ResetValues entry
        let s = format!("    pub {}: {},", reg_info.name, size_type_name);
        reset_values.push(s);
    }

    let mut access_types: Vec<_> = access_types.iter().map(|s| s.to_string()).collect();
    access_types.sort();
    writeln!(file, "use ral_registers::{{{}}};", access_types.join(", "))?;
    writeln!(file, "use core::marker::PhantomData;\n")?;

    writeln!(file, "{}", register_modules.join("\n"))?;

    writeln!(file, "pub struct RegisterBlock {{")?;
    write!(file, "{}", register_block.join("\n"))?;
    writeln!(file, "}}\n")?;

    writeln!(file, "pub struct ResetValues {{")?;
    writeln!(file, "{}", reset_values.join("\n"))?;
    writeln!(file, "}}")?;

    writeln!(
        file,
        "
pub struct Instance {{
    pub(crate) addr: {},
    pub(crate) _marker: PhantomData<*const RegisterBlock>,
}}

impl ::core::ops::Deref for Instance {{
    type Target = RegisterBlock;
    #[inline(always)]
    fn deref(&self) -> &RegisterBlock {{
        unsafe {{ &*(self.addr as *const _) }}
    }}
}}",
        config.address_size.type_name()
    )?;

    Ok(())
}

fn write_peripheral_instance(
    file: &mut fs::File,
    instance: &ModelPeripheralInstance,
    config: &GeneratorConfig,
) -> Result<()> {
    writeln!(file, "#![allow(non_snake_case, non_upper_case_globals)]")?;
    writeln!(file, "#![allow(non_camel_case_types)]")?;
    writeln!(file, "{}", build_doc_comment("//!", &instance.description))?;

    let peripheral_mod = &instance.peripheral_module;
    writeln!(file, "pub use super::super::peripherals::{}::Instance;", peripheral_mod)?;
    writeln!(
        file,
        "pub use super::super::peripherals::{}::{{RegisterBlock, ResetValues}};",
        peripheral_mod
    )?;

    let mut register_types = Vec::new();
    for value in &instance.reset_values {
        register_types.push(value.register.clone());
    }

    if !register_types.is_empty() {
        writeln!(
            file,
            "pub use super::super::peripherals::{}::{{{}}};",
            peripheral_mod,
            register_types.join(", ")
        )?;
    }
    writeln!(file)?;

    write!(
        file,
        "
/// Access functions for the {name} peripheral instance
pub mod {name} {{
    use super::ResetValues;
    use super::Instance;

    const INSTANCE: Instance = Instance {{
        addr: {:#x},
        _marker: ::core::marker::PhantomData,
    }};

    /// Reset values for each field in {name}
    pub const reset: ResetValues = ResetValues {{
",
        instance.base_address,
        name = instance.name
    )?;

    let mut values = Vec::new();
    for value in &instance.reset_values {
        values.push(format!("        {}: {:#x},", value.register, value.value));
    }
    write!(file, "{}", values.join("\n"))?;

    writeln!(
        file,
        "
    }};

    #[allow(renamed_and_removed_lints)]
    #[allow(private_no_mangle_statics)]
    #[no_mangle]
    static mut {name}_TAKEN: bool = false;

    /// Safe access to {name}
    ///
    /// This function returns `Some(Instance)` if this instance is not
    /// currently taken, and `None` if it is. This ensures that if you
    /// do get `Some(Instance)`, you are ensured unique access to
    /// the peripheral and there cannot be data races (unless other
    /// code uses `unsafe`, of course). You can then pass the
    /// `Instance` around to other functions as required. When you're
    /// done with it, you can call `release(instance)` to return it.
    ///
    /// `Instance` itself dereferences to a `RegisterBlock`, which
    /// provides access to the peripheral's registers.
    #[inline]
    pub fn take() -> Option<Instance> {{
        {arch_crate}::interrupt::free(|_| unsafe {{
            if {name}_TAKEN {{
                None
            }} else {{
                {name}_TAKEN = true;
                Some(INSTANCE)
            }}
        }})
    }}

    /// Release exclusive access to {name}
    ///
    /// This function allows you to return an `Instance` so that it
    /// is available to `take()` again. This function will panic if
    /// you return a different `Instance` or if this instance is not
    /// already taken.
    #[inline]
    pub fn release(inst: Instance) {{
        {arch_crate}::interrupt::free(|_| unsafe {{
            if {name}_TAKEN && inst.addr == INSTANCE.addr {{
                {name}_TAKEN = false;
            }} else {{
                panic!(\"Released a peripheral which was not taken\");
            }}
        }});
    }}

    /// Unsafely steal {name}
    ///
    /// This function is similar to take() but forcibly takes the
    /// Instance, marking it as taken irregardless of its previous
    /// state.
    #[allow(clippy::missing_safety_doc)]
    #[inline]
    pub unsafe fn steal() -> Instance {{
        {name}_TAKEN = true;
        INSTANCE
    }}

    /// Unsafely obtains an instance of {name}
    ///
    /// This will not check if `take()` or `steal()` have already been called
    /// before. It is the caller's responsibility to use the returned instance
    /// in a safe way that does not conflict with other instances.
    #[allow(clippy::missing_safety_doc)]
    #[inline]
    pub unsafe fn conjure() -> Instance {{
        INSTANCE
    }}
}}

/// Raw pointer to {name}
///
/// Dereferencing this is unsafe because you are not ensured unique
/// access to the peripheral, so you may encounter data races with
/// other users of this peripheral. It is up to you to ensure you
/// will not cause data races.
///
/// This constant is provided for ease of use in unsafe code: you can
/// simply call for example `write_reg!(gpio, GPIOA, ODR, 1);`.
pub const {name}: *const RegisterBlock = {:#x} as *const _;",
        instance.base_address,
        arch_crate = config.arch_crate,
        name = instance.name
    )?;

    Ok(())
}

trait Codegen {
    fn generate_code(&self) -> String;
}

impl Codegen for FinalFieldInfo {
    fn generate_code(&self) -> String {
        let mut code = String::new();
        if let Some(descrition) = self.description.as_ref() {
            code = build_doc_comment("///", descrition);
        }

        writeln!(code, "pub mod {} {{", self.name).unwrap();

        writeln!(code, "    /// Offset ({} bits)", self.bit_range.offset).unwrap();
        writeln!(code, "    pub const offset: u32 = {};", self.bit_range.offset).unwrap();

        eprintln!("{} ({})", self.bit_range.width, self.name);
        let mask = (1u64 << self.bit_range.width) - 1;
        write!(
            code,
            "
    /// Mask ({} bit: {:#x} << {})
    pub const mask: u32 = {:#x} << offset;
",
            self.bit_range.width, mask, self.bit_range.offset, mask
        )
        .unwrap();

        writeln!(
            code,
            "
    /// Read-only values (empty)
    pub mod R {{}}
    /// Write-only values (empty)
    pub mod W {{}}
    /// Read-write values (empty)
    pub mod RW {{}}
"
        )
        .unwrap();

        writeln!(code, "}}").unwrap();

        indent(&code, 1)
    }
}

fn build_doc_comment(prefix: &str, doc: &str) -> String {
    let mut doc_string = String::new();
    for line in doc.lines() {
        writeln!(doc_string, "{} {}", prefix, line).unwrap();
    }
    doc_string
}

fn indent(s: &str, levels: usize) -> String {
    let prefix = "    ".repeat(levels);

    let mut lines = Vec::new();
    for line in s.lines() {
        lines.push(prefix.clone() + line);
    }

    lines.join("\n")
}
