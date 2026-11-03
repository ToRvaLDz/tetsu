//! What each method of the contract is gated by: its class, the parameter
//! that scopes a grant, and whether it is destructive. Written by hand and
//! cross-checked against the IDL tags by `tests/catalog.rs`, so that
//! `tetsu-bus-contract` is linked into no daemon.

/// One method of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Fully qualified method name.
    pub method: &'static str,
    /// The `@capability` class; `None` for a method nothing gates.
    pub class: Option<&'static str>,
    /// The input parameter that scopes the grant.
    pub scope_param: Option<&'static str>,
    /// Whether the method is `@destructive`.
    pub destructive: bool,
}

const fn entry(
    method: &'static str,
    class: Option<&'static str>,
    scope_param: Option<&'static str>,
    destructive: bool,
) -> Entry {
    Entry {
        method,
        class,
        scope_param,
        destructive,
    }
}

/// Every method of every interface.
pub const ENTRIES: &[Entry] = &[
    entry("org.tetsu.cap.v1.ListCapabilities", None, None, false),
    entry("org.tetsu.cap.v1.WatchCapabilities", None, None, false),
    entry("org.tetsu.cap.v1.Set", None, None, false),
    entry("org.tetsu.cap.v1.Check", None, None, false),
    entry("org.tetsu.cap.v1.Intent", None, None, false),
    entry("org.tetsu.comp.clipboard.v1.ListOffers", None, None, false),
    entry(
        "org.tetsu.comp.clipboard.v1.WatchClipboard",
        None,
        None,
        false,
    ),
    entry("org.tetsu.comp.config.v1.GetConfig", None, None, false),
    entry("org.tetsu.comp.config.v1.WatchConfig", None, None, false),
    entry(
        "org.tetsu.comp.config.v1.SetManaged",
        Some("comp.configure"),
        None,
        false,
    ),
    entry("org.tetsu.comp.output.v1.ListOutputs", None, None, false),
    entry("org.tetsu.comp.output.v1.WatchOutputs", None, None, false),
    entry(
        "org.tetsu.comp.output.v1.ConfigureOutputs",
        Some("display.configure"),
        None,
        false,
    ),
    entry("org.tetsu.comp.render.v1.GetRender", None, None, false),
    entry("org.tetsu.comp.render.v1.WatchRender", None, None, false),
    entry(
        "org.tetsu.comp.render.v1.ListRenderStats",
        None,
        None,
        false,
    ),
    entry(
        "org.tetsu.comp.render.v1.WatchRenderStats",
        None,
        None,
        false,
    ),
    entry(
        "org.tetsu.comp.render.v1.SetRenderDevice",
        Some("display.render"),
        None,
        false,
    ),
    entry(
        "org.tetsu.comp.render.v1.SetExplicitSync",
        Some("display.render"),
        None,
        false,
    ),
    entry("org.tetsu.comp.session.v1.GetSession", None, None, false),
    entry("org.tetsu.comp.session.v1.WatchSession", None, None, false),
    entry("org.tetsu.comp.window.v1.ListWindows", None, None, false),
    entry("org.tetsu.comp.window.v1.WatchWindows", None, None, false),
    entry(
        "org.tetsu.comp.workspace.v1.ListWorkspaces",
        None,
        None,
        false,
    ),
    entry(
        "org.tetsu.comp.workspace.v1.WatchWorkspaces",
        None,
        None,
        false,
    ),
    entry("org.tetsu.notify.v1.Show", Some("notify.send"), None, false),
    entry("org.tetsu.notify.v1.GetPending", None, None, false),
    entry("org.tetsu.notify.v1.WatchPending", None, None, false),
    entry(
        "org.tetsu.notify.v1.OpenPanel",
        Some("notify.send"),
        None,
        false,
    ),
    entry(
        "org.tetsu.notify.v1.Close",
        Some("notify.send"),
        None,
        false,
    ),
    entry("org.tetsu.secrets.v1.ListSecrets", None, None, false),
    entry("org.tetsu.secrets.v1.WatchSecrets", None, None, false),
    entry(
        "org.tetsu.secrets.v1.Add",
        Some("secrets.manage"),
        None,
        false,
    ),
    entry(
        "org.tetsu.secrets.v1.Rotate",
        Some("secrets.manage"),
        None,
        true,
    ),
    entry(
        "org.tetsu.secrets.v1.Remove",
        Some("secrets.manage"),
        None,
        true,
    ),
    entry("org.tetsu.tx.v1.Update", Some("system.update"), None, true),
    entry(
        "org.tetsu.tx.v1.Install",
        Some("packages.install"),
        None,
        false,
    ),
    entry(
        "org.tetsu.tx.v1.Remove",
        Some("packages.remove"),
        None,
        false,
    ),
    entry(
        "org.tetsu.tx.v1.Rollback",
        Some("system.rollback"),
        None,
        false,
    ),
    entry(
        "org.tetsu.tx.v1.ReviewAurPackages",
        Some("aur.review"),
        None,
        false,
    ),
    entry(
        "org.tetsu.tx.v1.InstallFromAur",
        Some("aur.install"),
        None,
        true,
    ),
    entry("org.tetsu.tx.v1.ListTransactions", None, None, false),
    entry("org.tetsu.tx.v1.ListUserPackages", None, None, false),
    entry("org.tetsu.tx.v1.GetStatus", None, None, false),
    entry("org.tetsu.tx.v1.WatchTransactions", None, None, false),
];

/// One class and the methods it gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    /// The class name.
    pub name: &'static str,
    /// The fully qualified methods of the class.
    pub methods: Vec<&'static str>,
    /// Whether any method of the class is `@destructive`.
    pub destructive: bool,
    /// The parameter that scopes a grant, when the class has one.
    pub scope_param: Option<&'static str>,
}

/// The entry of a method, if the contract has it.
pub fn lookup(method: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.method == method)
}

/// The classes in first-seen order, each with its methods.
pub fn classes() -> Vec<Class> {
    let mut out: Vec<Class> = Vec::new();
    for entry in ENTRIES {
        let Some(name) = entry.class else { continue };
        match out.iter_mut().find(|c| c.name == name) {
            Some(class) => {
                class.methods.push(entry.method);
                class.destructive |= entry.destructive;
                class.scope_param = class.scope_param.or(entry.scope_param);
            }
            None => out.push(Class {
                name,
                methods: vec![entry.method],
                destructive: entry.destructive,
                scope_param: entry.scope_param,
            }),
        }
    }
    out
}

/// The class of that name, if any method has it.
pub fn class(name: &str) -> Option<Class> {
    classes().into_iter().find(|c| c.name == name)
}

/// Whether some method is gated by this class.
pub fn is_class(name: &str) -> bool {
    ENTRIES.iter().any(|e| e.class == Some(name))
}
