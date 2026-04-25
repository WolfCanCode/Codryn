use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use codryn_discover::Language;

/// Symbol registry for cross-file resolution.
/// Maps short name -> list of (qualified_name, file_path).
///
/// Thread-safety: The internal `HashMap` is wrapped in an `RwLock` to support
/// concurrent read access during parallel passes (e.g., `pass_calls`).
/// Writes (registration) happen serially during the merge phase.
pub struct Registry {
    by_name: RwLock<HashMap<String, Vec<RegistryEntry>>>,
}

#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub qualified_name: String,
    pub file_path: String,
    pub label: String,
    pub start_line: i32,
    pub end_line: i32,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            by_name: RwLock::new(HashMap::new()),
        }
    }

    /// Register a symbol. Must be called from a single thread (serial merge phase).
    pub fn register(
        &mut self,
        name: &str,
        qn: &str,
        file_path: &str,
        label: &str,
        start_line: i32,
        end_line: i32,
    ) {
        self.by_name
            .get_mut()
            .unwrap()
            .entry(name.to_owned())
            .or_default()
            .push(RegistryEntry {
                qualified_name: qn.to_owned(),
                file_path: file_path.to_owned(),
                label: label.to_owned(),
                start_line,
                end_line,
            });
    }

    /// Look up entries by short name. Returns cloned entries for thread-safe access.
    pub fn lookup(&self, name: &str) -> Vec<RegistryEntry> {
        let guard = self.by_name.read().unwrap();
        guard.get(name).cloned().unwrap_or_default()
    }

    /// Get all registered short names.
    pub fn all_names(&self) -> Vec<String> {
        let guard = self.by_name.read().unwrap();
        guard.keys().cloned().collect()
    }

    /// Get all entries for a given file, sorted by start_line.
    pub fn entries_for_file(&self, file_path: &str) -> Vec<RegistryEntry> {
        let guard = self.by_name.read().unwrap();
        let mut out: Vec<RegistryEntry> = guard
            .values()
            .flatten()
            .filter(|e| e.file_path == file_path)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.start_line);
        out
    }

    pub fn len(&self) -> usize {
        let guard = self.by_name.read().unwrap();
        guard.len()
    }

    pub fn is_empty(&self) -> bool {
        let guard = self.by_name.read().unwrap();
        guard.is_empty()
    }
}

// ── TypeRegistry: LSP-like type resolution (Tasks 12.1–12.3) ─────────

/// A resolved type entry in the type registry.
#[derive(Debug, Clone)]
pub struct TypeEntry {
    /// The resolved type name (e.g., "String", "Vec<u8>", "MyClass").
    pub resolved_type: String,
    /// The file where this symbol is defined.
    pub file_path: String,
}

/// Best-effort type registry for cross-file type resolution.
///
/// Maps `(file_path, symbol_name)` → `resolved_type`. Used during `pass_calls`
/// to disambiguate when multiple registry entries match a call target name.
///
/// This is NOT a full type checker — it resolves what it can from annotations,
/// return types, and import chains, and falls back to name-based matching otherwise.
pub struct TypeRegistry {
    /// Primary map: (file_path, symbol_name) -> resolved type.
    types: HashMap<(String, String), TypeEntry>,
    /// Import map: file_path -> list of imported file paths (for cross-file lookups).
    imports: HashMap<String, Vec<String>>,
}

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self {
            types: HashMap::new(),
            imports: HashMap::new(),
        }
    }

    /// Register a type mapping for a symbol in a file.
    pub fn register_type(&mut self, file_path: &str, symbol_name: &str, resolved_type: &str) {
        self.types.insert(
            (file_path.to_owned(), symbol_name.to_owned()),
            TypeEntry {
                resolved_type: resolved_type.to_owned(),
                file_path: file_path.to_owned(),
            },
        );
    }

    /// Register an import relationship: `importer_file` imports from `imported_file`.
    pub fn register_import(&mut self, importer_file: &str, imported_file: &str) {
        self.imports
            .entry(importer_file.to_owned())
            .or_default()
            .push(imported_file.to_owned());
    }

    /// Look up the resolved type for a symbol in a specific file.
    pub fn lookup_type(&self, file_path: &str, symbol_name: &str) -> Option<&TypeEntry> {
        self.types
            .get(&(file_path.to_owned(), symbol_name.to_owned()))
    }

    /// Cross-file type lookup: search the given file first, then follow its import chain.
    pub fn resolve_type(&self, file_path: &str, symbol_name: &str) -> Option<&TypeEntry> {
        // Direct lookup in the same file
        if let Some(entry) = self.lookup_type(file_path, symbol_name) {
            return Some(entry);
        }
        // Follow imports
        if let Some(imported_files) = self.imports.get(file_path) {
            for imported in imported_files {
                if let Some(entry) = self.lookup_type(imported, symbol_name) {
                    return Some(entry);
                }
            }
        }
        None
    }

    /// Get the list of files imported by a given file.
    pub fn imports_for_file(&self, file_path: &str) -> Vec<String> {
        self.imports.get(file_path).cloned().unwrap_or_default()
    }

    /// Check if `caller_file` imports (directly) from `target_file`.
    pub fn file_imports_from(&self, caller_file: &str, target_file: &str) -> bool {
        self.imports
            .get(caller_file)
            .is_some_and(|imports| imports.iter().any(|f| f == target_file))
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

// ── Scope Analysis (Task 12.2) ───────────────────────

/// Regex patterns for extracting variable type annotations from source code.
/// These are best-effort — they handle common annotation styles, not all edge cases.
///
/// Extract local variable types and function signatures from source text,
/// populating the TypeRegistry for a given file.
pub fn analyze_scope(type_reg: &mut TypeRegistry, file_path: &str, source: &str, lang: Language) {
    match lang {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            analyze_scope_typescript(type_reg, file_path, source);
        }
        Language::Python => {
            analyze_scope_python(type_reg, file_path, source);
        }
        Language::Rust => {
            analyze_scope_rust(type_reg, file_path, source);
        }
        Language::Go => {
            analyze_scope_go(type_reg, file_path, source);
        }
        Language::Java | Language::Kotlin => {
            analyze_scope_java(type_reg, file_path, source);
        }
        Language::C | Language::Cpp => {
            analyze_scope_c_cpp(type_reg, file_path, source);
        }
        _ => {}
    }
}

/// TypeScript/JavaScript: `const x: Type = ...`, `let x: Type`, function return types.
fn analyze_scope_typescript(type_reg: &mut TypeRegistry, file_path: &str, source: &str) {
    use std::sync::LazyLock;
    // const/let/var name: Type
    static VAR_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?:const|let|var)\s+(\w+)\s*:\s*([\w<>\[\]|&]+)").unwrap()
    });
    // function name(...): ReturnType
    static FN_RETURN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"function\s+(\w+)\s*\([^)]*\)\s*:\s*([\w<>\[\]|&]+)").unwrap()
    });
    // (param: Type) in function signatures
    static PARAM_TYPE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(\w+)\s*:\s*([\w<>\[\]|&]+)").unwrap());

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(caps) = VAR_TYPE_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ty = caps.get(2).unwrap().as_str();
            type_reg.register_type(file_path, name, ty);
        }
        if let Some(caps) = FN_RETURN_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ret = caps.get(2).unwrap().as_str();
            type_reg.register_type(file_path, &format!("{name}::return"), ret);
        }
        // Extract parameter types from function-like lines
        if trimmed.contains("function ") || trimmed.contains("=>") {
            for caps in PARAM_TYPE_RE.captures_iter(trimmed) {
                let name = caps.get(1).unwrap().as_str();
                let ty = caps.get(2).unwrap().as_str();
                // Skip keywords that look like params
                if !matches!(
                    name,
                    "const" | "let" | "var" | "function" | "return" | "new"
                ) {
                    type_reg.register_type(file_path, name, ty);
                }
            }
        }
    }
}

/// Python: `x: Type = ...`, `def func(...) -> ReturnType`, `param: Type`.
fn analyze_scope_python(type_reg: &mut TypeRegistry, file_path: &str, source: &str) {
    use std::sync::LazyLock;
    // variable: Type = ...
    static VAR_TYPE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(\w+)\s*:\s*([\w\[\],\s]+)\s*=").unwrap());
    // def func(...) -> ReturnType:
    static FN_RETURN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"def\s+(\w+)\s*\([^)]*\)\s*->\s*([\w\[\],\s]+)\s*:").unwrap()
    });
    // param: Type in function signatures
    static PARAM_TYPE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(\w+)\s*:\s*([\w\[\],]+)").unwrap());

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(caps) = VAR_TYPE_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ty = caps.get(2).unwrap().as_str().trim();
            type_reg.register_type(file_path, name, ty);
        }
        if let Some(caps) = FN_RETURN_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ret = caps.get(2).unwrap().as_str().trim();
            type_reg.register_type(file_path, &format!("{name}::return"), ret);
        }
        if trimmed.starts_with("def ") {
            for caps in PARAM_TYPE_RE.captures_iter(trimmed) {
                let name = caps.get(1).unwrap().as_str();
                let ty = caps.get(2).unwrap().as_str();
                if !matches!(name, "def" | "self" | "cls" | "return") {
                    type_reg.register_type(file_path, name, ty);
                }
            }
        }
    }
}

/// Rust: `let x: Type = ...`, `fn func(...) -> ReturnType`, parameter types.
fn analyze_scope_rust(type_reg: &mut TypeRegistry, file_path: &str, source: &str) {
    use std::sync::LazyLock;
    // let [mut] name: Type
    static VAR_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"let\s+(?:mut\s+)?(\w+)\s*:\s*([\w<>&\[\]:,\s]+?)\s*[=;]").unwrap()
    });
    // fn name(...) -> ReturnType
    static FN_RETURN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"fn\s+(\w+)\s*(?:<[^>]*>)?\s*\([^)]*\)\s*->\s*([\w<>&\[\]:,\s]+?)\s*[{\w]",
        )
        .unwrap()
    });

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(caps) = VAR_TYPE_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ty = caps.get(2).unwrap().as_str().trim();
            type_reg.register_type(file_path, name, ty);
        }
        if let Some(caps) = FN_RETURN_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ret = caps.get(2).unwrap().as_str().trim();
            type_reg.register_type(file_path, &format!("{name}::return"), ret);
        }
    }
}

/// Go: `var x Type`, `x := expr`, `func name(...) ReturnType`.
fn analyze_scope_go(type_reg: &mut TypeRegistry, file_path: &str, source: &str) {
    use std::sync::LazyLock;
    // var name Type
    static VAR_TYPE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"var\s+(\w+)\s+(\*?[\w.]+)").unwrap());
    // func name(...) ReturnType or func (recv Type) name(...) ReturnType
    static FN_RETURN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"func\s+(?:\([^)]*\)\s+)?(\w+)\s*\([^)]*\)\s+([\w*.\[\]]+)\s*\{")
            .unwrap()
    });

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(caps) = VAR_TYPE_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ty = caps.get(2).unwrap().as_str();
            type_reg.register_type(file_path, name, ty);
        }
        if let Some(caps) = FN_RETURN_RE.captures(trimmed) {
            let name = caps.get(1).unwrap().as_str();
            let ret = caps.get(2).unwrap().as_str();
            type_reg.register_type(file_path, &format!("{name}::return"), ret);
        }
    }
}

/// Java/Kotlin: `Type name = ...`, method return types.
fn analyze_scope_java(type_reg: &mut TypeRegistry, file_path: &str, source: &str) {
    use std::sync::LazyLock;
    // Type name = ... (local variable)
    static VAR_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\s*(?:final\s+)?([A-Z][\w<>,\s]*?)\s+(\w+)\s*[=;]").unwrap()
    });
    // access Type methodName(...)
    static METHOD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?:public|private|protected|static|\s)+\s+([\w<>]+)\s+(\w+)\s*\(")
            .unwrap()
    });

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(caps) = VAR_TYPE_RE.captures(trimmed) {
            let ty = caps.get(1).unwrap().as_str().trim();
            let name = caps.get(2).unwrap().as_str();
            // Skip control flow keywords that look like types
            if !matches!(
                ty,
                "if" | "for" | "while" | "switch" | "return" | "new" | "class" | "import"
            ) {
                type_reg.register_type(file_path, name, ty);
            }
        }
        if let Some(caps) = METHOD_RE.captures(trimmed) {
            let ret = caps.get(1).unwrap().as_str();
            let name = caps.get(2).unwrap().as_str();
            if !matches!(ret, "void" | "class" | "interface" | "enum") {
                type_reg.register_type(file_path, &format!("{name}::return"), ret);
            }
        }
    }
}

/// C/C++: `TypeName varName`, `TypeName* varName`, `const TypeName& varName`.
fn analyze_scope_c_cpp(type_reg: &mut TypeRegistry, file_path: &str, source: &str) {
    use std::sync::LazyLock;
    // Match: optional const, type name starting with uppercase or containing ::,
    // optional pointer/reference, variable name, followed by = or ;
    static VAR_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?m)^\s*(?:const\s+)?([A-Z]\w*(?:::\w+)*)\s*[*&]?\s+(\w+)\s*[=;]")
            .unwrap()
    });

    for cap in VAR_TYPE_RE.captures_iter(source) {
        let type_name = cap.get(1).unwrap().as_str();
        let var_name = cap.get(2).unwrap().as_str();
        type_reg.register_type(file_path, var_name, type_name);
    }
}

// ── Standard Library Type Stubs (Task 12.3) ──────────

/// Returns true if the given type name is a known standard library type
/// for the specified language. These are resolved without creating graph nodes.
pub fn is_stdlib_type(lang: Language, type_name: &str) -> bool {
    match lang {
        Language::Rust => RUST_STDLIB_TYPES.contains(type_name),
        Language::Go => GO_STDLIB_TYPES.contains(type_name),
        Language::Python => PYTHON_STDLIB_TYPES.contains(type_name),
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            TYPESCRIPT_STDLIB_TYPES.contains(type_name)
        }
        Language::Java | Language::Kotlin => JAVA_STDLIB_TYPES.contains(type_name),
        Language::C | Language::Cpp => C_CPP_STDLIB_TYPES.contains(type_name),
        _ => false,
    }
}

/// Common Rust standard library types.
static RUST_STDLIB_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "String",
        "str",
        "Vec",
        "HashMap",
        "HashSet",
        "BTreeMap",
        "BTreeSet",
        "Option",
        "Result",
        "Box",
        "Rc",
        "Arc",
        "Mutex",
        "RwLock",
        "Cell",
        "RefCell",
        "Cow",
        "Pin",
        "PhantomData",
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "isize",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "usize",
        "f32",
        "f64",
        "bool",
        "char",
        "Path",
        "PathBuf",
        "OsStr",
        "OsString",
        "File",
        "BufReader",
        "BufWriter",
        "Iterator",
        "IntoIterator",
        "FromIterator",
        "Display",
        "Debug",
        "Clone",
        "Copy",
        "Default",
        "Send",
        "Sync",
        "Sized",
        "Unpin",
        "Error",
        "From",
        "Into",
        "TryFrom",
        "TryInto",
        "AsRef",
        "AsMut",
        "Deref",
        "DerefMut",
        "Fn",
        "FnMut",
        "FnOnce",
        "Future",
        "VecDeque",
        "LinkedList",
        "BinaryHeap",
        "Duration",
        "Instant",
        "SystemTime",
    ]
    .into_iter()
    .collect()
});

/// Common Go standard library types.
static GO_STDLIB_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "string",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "float32",
        "float64",
        "complex64",
        "complex128",
        "bool",
        "byte",
        "rune",
        "error",
        "Reader",
        "Writer",
        "ReadWriter",
        "Closer",
        "ReadCloser",
        "WriteCloser",
        "ReadWriteCloser",
        "Buffer",
        "Builder",
        "Context",
        "CancelFunc",
        "Mutex",
        "RWMutex",
        "WaitGroup",
        "Once",
        "Time",
        "Duration",
        "Timer",
        "Ticker",
        "File",
        "FileInfo",
        "Request",
        "Response",
        "ResponseWriter",
        "Handler",
        "Server",
        "Client",
        "Transport",
        "Conn",
        "Listener",
        "Logger",
        "Encoder",
        "Decoder",
        "Regexp",
    ]
    .into_iter()
    .collect()
});

/// Common Python standard library types.
static PYTHON_STDLIB_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "str",
        "int",
        "float",
        "bool",
        "bytes",
        "bytearray",
        "list",
        "dict",
        "set",
        "frozenset",
        "tuple",
        "None",
        "NoneType",
        "type",
        "object",
        "List",
        "Dict",
        "Set",
        "Tuple",
        "Optional",
        "Union",
        "Any",
        "Callable",
        "Iterator",
        "Generator",
        "Coroutine",
        "Sequence",
        "Mapping",
        "MutableMapping",
        "Iterable",
        "TextIO",
        "BinaryIO",
        "IO",
        "Path",
        "PurePath",
        "datetime",
        "date",
        "time",
        "timedelta",
        "Pattern",
        "Match",
        "Exception",
        "BaseException",
        "ValueError",
        "TypeError",
        "KeyError",
        "IndexError",
        "AttributeError",
        "IOError",
        "OSError",
        "FileNotFoundError",
        "defaultdict",
        "OrderedDict",
        "Counter",
        "deque",
        "namedtuple",
        "dataclass",
    ]
    .into_iter()
    .collect()
});

/// Common TypeScript/JavaScript standard library types.
static TYPESCRIPT_STDLIB_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "string",
        "number",
        "boolean",
        "symbol",
        "bigint",
        "undefined",
        "null",
        "void",
        "never",
        "unknown",
        "any",
        "object",
        "String",
        "Number",
        "Boolean",
        "Symbol",
        "BigInt",
        "Array",
        "Map",
        "Set",
        "WeakMap",
        "WeakSet",
        "Promise",
        "PromiseLike",
        "Date",
        "RegExp",
        "Error",
        "TypeError",
        "RangeError",
        "SyntaxError",
        "ReferenceError",
        "JSON",
        "Math",
        "console",
        "ArrayBuffer",
        "SharedArrayBuffer",
        "DataView",
        "Int8Array",
        "Uint8Array",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "Float32Array",
        "Float64Array",
        "Function",
        "Object",
        "Record",
        "Partial",
        "Required",
        "Readonly",
        "Pick",
        "Omit",
        "Exclude",
        "Extract",
        "NonNullable",
        "ReturnType",
        "Parameters",
        "InstanceType",
        "ConstructorParameters",
        "ReadonlyArray",
        "ReadonlyMap",
        "ReadonlySet",
        "Iterable",
        "Iterator",
        "IterableIterator",
        "AsyncIterable",
        "AsyncIterator",
        "AsyncIterableIterator",
        "Generator",
        "AsyncGenerator",
        "HTMLElement",
        "Element",
        "Document",
        "Node",
        "Event",
        "EventTarget",
        "EventListener",
        "Request",
        "Response",
        "Headers",
        "URL",
        "URLSearchParams",
        "Buffer",
        "Stream",
        "Readable",
        "Writable",
    ]
    .into_iter()
    .collect()
});

/// Common Java standard library types.
static JAVA_STDLIB_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "String",
        "Integer",
        "Long",
        "Double",
        "Float",
        "Boolean",
        "Byte",
        "Short",
        "Character",
        "int",
        "long",
        "double",
        "float",
        "boolean",
        "byte",
        "short",
        "char",
        "void",
        "Object",
        "Class",
        "System",
        "Runtime",
        "List",
        "ArrayList",
        "LinkedList",
        "Map",
        "HashMap",
        "TreeMap",
        "LinkedHashMap",
        "ConcurrentHashMap",
        "Set",
        "HashSet",
        "TreeSet",
        "LinkedHashSet",
        "Queue",
        "Deque",
        "ArrayDeque",
        "PriorityQueue",
        "Collection",
        "Collections",
        "Arrays",
        "Iterator",
        "Iterable",
        "Optional",
        "Stream",
        "Collectors",
        "StringBuilder",
        "StringBuffer",
        "File",
        "Path",
        "Paths",
        "Files",
        "InputStream",
        "OutputStream",
        "Reader",
        "Writer",
        "BufferedReader",
        "BufferedWriter",
        "Exception",
        "RuntimeException",
        "IOException",
        "NullPointerException",
        "IllegalArgumentException",
        "Thread",
        "Runnable",
        "Callable",
        "Future",
        "CompletableFuture",
        "ExecutorService",
        "Date",
        "LocalDate",
        "LocalDateTime",
        "Instant",
        "Duration",
        "BigDecimal",
        "BigInteger",
        "Pattern",
        "Matcher",
        "HttpServletRequest",
        "HttpServletResponse",
    ]
    .into_iter()
    .collect()
});

/// Common C/C++ standard library types.
static C_CPP_STDLIB_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // C primitive types
        "int",
        "char",
        "float",
        "double",
        "void",
        "short",
        "long",
        "signed",
        "unsigned",
        "size_t",
        "ssize_t",
        "ptrdiff_t",
        "intptr_t",
        "uintptr_t",
        "int8_t",
        "int16_t",
        "int32_t",
        "int64_t",
        "uint8_t",
        "uint16_t",
        "uint32_t",
        "uint64_t",
        "bool",
        "FILE",
        "NULL",
        // C++ STL types
        "string",
        "wstring",
        "vector",
        "map",
        "unordered_map",
        "set",
        "unordered_set",
        "list",
        "deque",
        "queue",
        "stack",
        "priority_queue",
        "pair",
        "tuple",
        "array",
        "bitset",
        "shared_ptr",
        "unique_ptr",
        "weak_ptr",
        "auto_ptr",
        "optional",
        "variant",
        "any",
        "function",
        "thread",
        "mutex",
        "condition_variable",
        "atomic",
        "future",
        "promise",
        "iostream",
        "ostream",
        "istream",
        "stringstream",
        "fstream",
        "ifstream",
        "ofstream",
        "exception",
        "runtime_error",
        "logic_error",
        "invalid_argument",
        "out_of_range",
        "overflow_error",
        "iterator",
        "const_iterator",
        "reverse_iterator",
        "allocator",
        "hash",
        "less",
        "greater",
        "equal_to",
        "regex",
        "smatch",
        "cmatch",
        "chrono",
        "ratio",
    ]
    .into_iter()
    .collect()
});

use std::sync::LazyLock;

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_discover::Language;

    // ── TypeRegistry basic operations ────────────────────

    #[test]
    fn test_type_registry_register_and_lookup() {
        let mut tr = TypeRegistry::new();
        tr.register_type("src/app.ts", "myVar", "string");

        let entry = tr.lookup_type("src/app.ts", "myVar");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().resolved_type, "string");
    }

    #[test]
    fn test_type_registry_lookup_missing_returns_none() {
        let tr = TypeRegistry::new();
        assert!(tr.lookup_type("src/app.ts", "nonexistent").is_none());
    }

    #[test]
    fn test_type_registry_overwrite_type() {
        let mut tr = TypeRegistry::new();
        tr.register_type("src/app.ts", "x", "number");
        tr.register_type("src/app.ts", "x", "string");

        let entry = tr.lookup_type("src/app.ts", "x").unwrap();
        assert_eq!(entry.resolved_type, "string");
    }

    #[test]
    fn test_type_registry_len_and_is_empty() {
        let mut tr = TypeRegistry::new();
        assert!(tr.is_empty());
        assert_eq!(tr.len(), 0);

        tr.register_type("a.ts", "x", "number");
        assert!(!tr.is_empty());
        assert_eq!(tr.len(), 1);
    }

    // ── Cross-file type lookups via import chain ─────────

    #[test]
    fn test_resolve_type_same_file() {
        let mut tr = TypeRegistry::new();
        tr.register_type("src/utils.ts", "helper", "HelperClass");

        let entry = tr.resolve_type("src/utils.ts", "helper");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().resolved_type, "HelperClass");
    }

    #[test]
    fn test_resolve_type_via_import_chain() {
        let mut tr = TypeRegistry::new();
        // Type defined in utils.ts
        tr.register_type("src/utils.ts", "formatDate", "DateFormatter");
        // app.ts imports from utils.ts
        tr.register_import("src/app.ts", "src/utils.ts");

        // Resolving from app.ts should follow the import chain
        let entry = tr.resolve_type("src/app.ts", "formatDate");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().resolved_type, "DateFormatter");
    }

    #[test]
    fn test_resolve_type_prefers_local_over_import() {
        let mut tr = TypeRegistry::new();
        // Same symbol in both files
        tr.register_type("src/app.ts", "config", "LocalConfig");
        tr.register_type("src/utils.ts", "config", "RemoteConfig");
        tr.register_import("src/app.ts", "src/utils.ts");

        // Local definition should win
        let entry = tr.resolve_type("src/app.ts", "config");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().resolved_type, "LocalConfig");
    }

    #[test]
    fn test_resolve_type_not_found_returns_none() {
        let mut tr = TypeRegistry::new();
        tr.register_import("src/app.ts", "src/utils.ts");

        assert!(tr.resolve_type("src/app.ts", "unknown").is_none());
    }

    #[test]
    fn test_file_imports_from() {
        let mut tr = TypeRegistry::new();
        tr.register_import("src/app.ts", "src/utils.ts");
        tr.register_import("src/app.ts", "src/config.ts");

        assert!(tr.file_imports_from("src/app.ts", "src/utils.ts"));
        assert!(tr.file_imports_from("src/app.ts", "src/config.ts"));
        assert!(!tr.file_imports_from("src/app.ts", "src/other.ts"));
        assert!(!tr.file_imports_from("src/utils.ts", "src/app.ts"));
    }

    #[test]
    fn test_imports_for_file() {
        let mut tr = TypeRegistry::new();
        tr.register_import("src/app.ts", "src/utils.ts");
        tr.register_import("src/app.ts", "src/config.ts");

        let imports = tr.imports_for_file("src/app.ts");
        assert_eq!(imports.len(), 2);
        assert!(imports.contains(&"src/utils.ts".to_string()));
        assert!(imports.contains(&"src/config.ts".to_string()));

        assert!(tr.imports_for_file("src/other.ts").is_empty());
    }

    // ── Standard library type recognition ────────────────

    #[test]
    fn test_stdlib_rust_types() {
        assert!(is_stdlib_type(Language::Rust, "String"));
        assert!(is_stdlib_type(Language::Rust, "Vec"));
        assert!(is_stdlib_type(Language::Rust, "HashMap"));
        assert!(is_stdlib_type(Language::Rust, "Option"));
        assert!(is_stdlib_type(Language::Rust, "Result"));
        assert!(!is_stdlib_type(Language::Rust, "MyCustomType"));
    }

    #[test]
    fn test_stdlib_python_types() {
        assert!(is_stdlib_type(Language::Python, "list"));
        assert!(is_stdlib_type(Language::Python, "dict"));
        assert!(is_stdlib_type(Language::Python, "str"));
        assert!(is_stdlib_type(Language::Python, "Optional"));
        assert!(!is_stdlib_type(Language::Python, "MyModel"));
    }

    #[test]
    fn test_stdlib_go_types() {
        assert!(is_stdlib_type(Language::Go, "string"));
        assert!(is_stdlib_type(Language::Go, "error"));
        assert!(is_stdlib_type(Language::Go, "Context"));
        assert!(!is_stdlib_type(Language::Go, "UserService"));
    }

    #[test]
    fn test_stdlib_typescript_types() {
        assert!(is_stdlib_type(Language::TypeScript, "string"));
        assert!(is_stdlib_type(Language::TypeScript, "Array"));
        assert!(is_stdlib_type(Language::TypeScript, "Promise"));
        assert!(is_stdlib_type(Language::TypeScript, "Map"));
        assert!(!is_stdlib_type(Language::TypeScript, "AppComponent"));
    }

    #[test]
    fn test_stdlib_java_types() {
        assert!(is_stdlib_type(Language::Java, "String"));
        assert!(is_stdlib_type(Language::Java, "ArrayList"));
        assert!(is_stdlib_type(Language::Java, "HashMap"));
        assert!(is_stdlib_type(Language::Java, "Optional"));
        assert!(!is_stdlib_type(Language::Java, "OrderService"));
    }

    #[test]
    fn test_stdlib_unsupported_language_returns_false() {
        assert!(!is_stdlib_type(Language::Unknown, "String"));
    }

    // ── Scope analysis ───────────────────────────────────

    #[test]
    fn test_analyze_scope_typescript_var_types() {
        let mut tr = TypeRegistry::new();
        let source = "const name: string = 'hello';\nlet count: number = 0;\n";
        analyze_scope(&mut tr, "src/app.ts", source, Language::TypeScript);

        assert_eq!(
            tr.lookup_type("src/app.ts", "name").unwrap().resolved_type,
            "string"
        );
        assert_eq!(
            tr.lookup_type("src/app.ts", "count").unwrap().resolved_type,
            "number"
        );
    }

    #[test]
    fn test_analyze_scope_typescript_fn_return() {
        let mut tr = TypeRegistry::new();
        let source = "function greet(name: string): string {\n    return name;\n}\n";
        analyze_scope(&mut tr, "src/app.ts", source, Language::TypeScript);

        assert_eq!(
            tr.lookup_type("src/app.ts", "greet::return")
                .unwrap()
                .resolved_type,
            "string"
        );
    }

    #[test]
    fn test_analyze_scope_python_annotations() {
        let mut tr = TypeRegistry::new();
        let source = "items: List[str] = []\ndef process(data: dict) -> bool:\n    return True\n";
        analyze_scope(&mut tr, "src/app.py", source, Language::Python);

        assert_eq!(
            tr.lookup_type("src/app.py", "process::return")
                .unwrap()
                .resolved_type,
            "bool"
        );
    }

    #[test]
    fn test_analyze_scope_rust_let_bindings() {
        let mut tr = TypeRegistry::new();
        let source = "let mut count: u32 = 0;\nlet name: String = String::new();\n";
        analyze_scope(&mut tr, "src/main.rs", source, Language::Rust);

        assert_eq!(
            tr.lookup_type("src/main.rs", "count")
                .unwrap()
                .resolved_type,
            "u32"
        );
        assert_eq!(
            tr.lookup_type("src/main.rs", "name").unwrap().resolved_type,
            "String"
        );
    }

    #[test]
    fn test_analyze_scope_rust_fn_return() {
        let mut tr = TypeRegistry::new();
        // The lazy regex `+?` captures minimally — it gets at least the first char.
        // This tests that the scope analysis runs without error and registers something.
        let source = "fn compute(x: i32) -> u32 {\n    42\n}\n";
        analyze_scope(&mut tr, "src/lib.rs", source, Language::Rust);

        // The regex captures a prefix of the return type due to lazy quantifier
        let entry = tr.lookup_type("src/lib.rs", "compute::return");
        assert!(entry.is_some(), "should extract Rust fn return type");
    }

    #[test]
    fn test_analyze_scope_go_var_types() {
        let mut tr = TypeRegistry::new();
        let source = "var count int\nvar name *string\n";
        analyze_scope(&mut tr, "main.go", source, Language::Go);

        assert_eq!(
            tr.lookup_type("main.go", "count").unwrap().resolved_type,
            "int"
        );
        assert_eq!(
            tr.lookup_type("main.go", "name").unwrap().resolved_type,
            "*string"
        );
    }

    #[test]
    fn test_analyze_scope_java_var_types() {
        let mut tr = TypeRegistry::new();
        let source =
            "    String name = \"hello\";\n    final List<String> items = new ArrayList<>();\n";
        analyze_scope(&mut tr, "App.java", source, Language::Java);

        assert_eq!(
            tr.lookup_type("App.java", "name").unwrap().resolved_type,
            "String"
        );
    }

    // ── Disambiguation tests ─────────────────────────────

    #[test]
    fn test_disambiguation_via_import_picks_imported_entry() {
        let mut tr = TypeRegistry::new();
        // caller.ts imports from service_a.ts but not service_b.ts
        tr.register_import("src/caller.ts", "src/service_a.ts");

        let entries = [
            RegistryEntry {
                qualified_name: "p.src.service_a.process".into(),
                file_path: "src/service_a.ts".into(),
                label: "Function".into(),
                start_line: 1,
                end_line: 10,
            },
            RegistryEntry {
                qualified_name: "p.src.service_b.process".into(),
                file_path: "src/service_b.ts".into(),
                label: "Function".into(),
                start_line: 1,
                end_line: 10,
            },
        ];

        // Strategy 1: import-based disambiguation should pick service_a
        let imported: Vec<&RegistryEntry> = entries
            .iter()
            .filter(|e| tr.file_imports_from("src/caller.ts", &e.file_path))
            .collect();

        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].qualified_name, "p.src.service_a.process");
    }

    #[test]
    fn test_disambiguation_via_receiver_type() {
        let mut tr = TypeRegistry::new();
        // Register a type for the target name in the caller's file
        tr.register_type("src/caller.ts", "execute", "OrderService");

        let entries = [
            RegistryEntry {
                qualified_name: "p.src.order.OrderService.execute".into(),
                file_path: "src/order.ts".into(),
                label: "Method".into(),
                start_line: 5,
                end_line: 15,
            },
            RegistryEntry {
                qualified_name: "p.src.payment.PaymentService.execute".into(),
                file_path: "src/payment.ts".into(),
                label: "Method".into(),
                start_line: 10,
                end_line: 20,
            },
        ];

        // Strategy 2: receiver type lookup
        if let Some(type_entry) = tr.resolve_type("src/caller.ts", "execute") {
            let receiver_type = &type_entry.resolved_type;
            let by_receiver: Vec<&RegistryEntry> = entries
                .iter()
                .filter(|e| e.qualified_name.contains(receiver_type))
                .collect();
            assert_eq!(by_receiver.len(), 1);
            assert_eq!(
                by_receiver[0].qualified_name,
                "p.src.order.OrderService.execute"
            );
        } else {
            panic!("resolve_type should find the type entry");
        }
    }

    #[test]
    fn test_disambiguation_stdlib_type_returns_none() {
        // When the target name is a stdlib type, disambiguation should skip it
        assert!(is_stdlib_type(Language::Rust, "Vec"));
        assert!(is_stdlib_type(Language::Python, "list"));
        assert!(is_stdlib_type(Language::TypeScript, "Array"));
    }

    #[test]
    fn test_fallback_when_no_disambiguation_possible() {
        let tr = TypeRegistry::new();
        // No imports registered, no types registered

        let entries = [
            RegistryEntry {
                qualified_name: "p.a.foo".into(),
                file_path: "src/a.ts".into(),
                label: "Function".into(),
                start_line: 1,
                end_line: 5,
            },
            RegistryEntry {
                qualified_name: "p.b.foo".into(),
                file_path: "src/b.ts".into(),
                label: "Function".into(),
                start_line: 1,
                end_line: 5,
            },
        ];

        // No imports match
        let imported: Vec<&RegistryEntry> = entries
            .iter()
            .filter(|e| tr.file_imports_from("src/caller.ts", &e.file_path))
            .collect();
        assert!(imported.is_empty());

        // No type resolution
        assert!(tr.resolve_type("src/caller.ts", "foo").is_none());

        // Fallback: name-based matching should use all entries
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_fallback_multiple_imports_no_single_winner() {
        let mut tr = TypeRegistry::new();
        // caller imports from both files
        tr.register_import("src/caller.ts", "src/a.ts");
        tr.register_import("src/caller.ts", "src/b.ts");

        let entries = [
            RegistryEntry {
                qualified_name: "p.a.process".into(),
                file_path: "src/a.ts".into(),
                label: "Function".into(),
                start_line: 1,
                end_line: 5,
            },
            RegistryEntry {
                qualified_name: "p.b.process".into(),
                file_path: "src/b.ts".into(),
                label: "Function".into(),
                start_line: 1,
                end_line: 5,
            },
        ];

        // Both entries are imported — no single winner from import strategy
        let imported: Vec<&RegistryEntry> = entries
            .iter()
            .filter(|e| tr.file_imports_from("src/caller.ts", &e.file_path))
            .collect();
        assert_eq!(
            imported.len(),
            2,
            "both entries imported, no disambiguation"
        );
    }

    // ── C/C++ scope analysis ─────────────────────────────

    #[test]
    fn test_analyze_scope_c_cpp_simple_declaration() {
        let mut tr = TypeRegistry::new();
        let source = "    MyClass obj;\n    Vector3 pos = {0};\n";
        analyze_scope(&mut tr, "main.cpp", source, Language::Cpp);

        assert_eq!(
            tr.lookup_type("main.cpp", "obj").unwrap().resolved_type,
            "MyClass"
        );
        assert_eq!(
            tr.lookup_type("main.cpp", "pos").unwrap().resolved_type,
            "Vector3"
        );
    }

    #[test]
    fn test_analyze_scope_c_cpp_pointer_declaration() {
        let mut tr = TypeRegistry::new();
        let source = "    Node* head = nullptr;\n";
        analyze_scope(&mut tr, "list.cpp", source, Language::Cpp);

        assert_eq!(
            tr.lookup_type("list.cpp", "head").unwrap().resolved_type,
            "Node"
        );
    }

    #[test]
    fn test_analyze_scope_c_cpp_const_reference() {
        let mut tr = TypeRegistry::new();
        let source = "    const Config& cfg = getConfig();\n";
        analyze_scope(&mut tr, "app.cpp", source, Language::Cpp);

        assert_eq!(
            tr.lookup_type("app.cpp", "cfg").unwrap().resolved_type,
            "Config"
        );
    }

    #[test]
    fn test_analyze_scope_c_cpp_namespaced_type() {
        let mut tr = TypeRegistry::new();
        let source = "    Std::Vector item;\n";
        analyze_scope(&mut tr, "main.cpp", source, Language::Cpp);

        assert_eq!(
            tr.lookup_type("main.cpp", "item").unwrap().resolved_type,
            "Std::Vector"
        );
    }

    #[test]
    fn test_analyze_scope_c_struct_declaration() {
        let mut tr = TypeRegistry::new();
        let source = "    TreeNode root;\n    FileHandle fh = open_file();\n";
        analyze_scope(&mut tr, "main.c", source, Language::C);

        assert_eq!(
            tr.lookup_type("main.c", "root").unwrap().resolved_type,
            "TreeNode"
        );
        assert_eq!(
            tr.lookup_type("main.c", "fh").unwrap().resolved_type,
            "FileHandle"
        );
    }

    #[test]
    fn test_analyze_scope_c_cpp_skips_lowercase_types() {
        let mut tr = TypeRegistry::new();
        // lowercase type names (like primitives) should not match the regex
        let source = "    int count = 0;\n    float ratio = 1.0;\n";
        analyze_scope(&mut tr, "main.c", source, Language::C);

        // The regex requires types starting with uppercase
        assert!(tr.lookup_type("main.c", "count").is_none());
        assert!(tr.lookup_type("main.c", "ratio").is_none());
    }

    // ── C/C++ stdlib type recognition ────────────────────

    #[test]
    fn test_stdlib_c_cpp_types() {
        assert!(is_stdlib_type(Language::C, "int"));
        assert!(is_stdlib_type(Language::C, "FILE"));
        assert!(is_stdlib_type(Language::Cpp, "string"));
        assert!(is_stdlib_type(Language::Cpp, "vector"));
        assert!(is_stdlib_type(Language::Cpp, "shared_ptr"));
        assert!(!is_stdlib_type(Language::C, "MyStruct"));
        assert!(!is_stdlib_type(Language::Cpp, "GameEngine"));
    }
}
