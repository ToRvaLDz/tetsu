//! Derivazione delle azioni polkit dalle classi `@capability`.

use std::collections::BTreeMap;
use std::fmt;

use crate::model::Contract;

/// Prefisso degli id azione: un solo namespace, un solo file.
const ACTION_PREFIX: &str = "org.tetsu.";

/// Un tag `@capability` malformato, o nessuna classe nel contratto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// Il tag del metodo non segue la grammatica.
    Malformed { method: String, reason: String },
    /// Nessuna classe: una policy vuota non va mai spedita.
    NoClasses,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyError::Malformed { method, reason } => {
                write!(f, "{method}: malformed @capability: {reason}")
            }
            PolicyError::NoClasses => f.write_str("the contract has no @capability class"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// L'id polkit di una classe: `_` diventa `-`, perche' polkit ammette solo
/// lettere, cifre, `.` e `-`. La mappa e' iniettiva: la classe non ha `-`.
pub fn polkit_action_id(class: &str) -> String {
    format!("{ACTION_PREFIX}{}", class.replace('_', "-"))
}

/// Le classi distinte con i metodi (qualificati) che le richiedono, in
/// ordine. Un tag malformato e' un errore.
pub fn capability_classes(
    contract: &Contract,
) -> Result<BTreeMap<String, Vec<String>>, PolicyError> {
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (iface, interface) in &contract.interfaces {
        for (name, method) in &interface.methods {
            let qualified = format!("{iface}.{name}");
            match method.doc.capability() {
                None => {}
                Some(Err(reason)) => {
                    return Err(PolicyError::Malformed {
                        method: qualified,
                        reason,
                    });
                }
                Some(Ok(cap)) => classes.entry(cap.class).or_default().push(qualified),
            }
        }
    }
    Ok(classes)
}

/// Il file `.policy` con un'azione per classe, tutte `auth_admin`.
pub fn polkit_policy(contract: &Contract) -> Result<String, PolicyError> {
    let classes = capability_classes(contract)?;
    if classes.is_empty() {
        return Err(PolicyError::NoClasses);
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE policyconfig PUBLIC \"-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN\"\n \
         \"http://www.freedesktop.org/standards/PolicyKit/1.0/policyconfig.dtd\">\n\
         <policyconfig>\n  <vendor>Tetsu</vendor>\n",
    );
    for (class, methods) in &classes {
        let text = format!("{class}: {}", methods.join(", "));
        // Classi e identificatori IDL non contengono metacaratteri XML.
        debug_assert!(!text.contains(['<', '>', '&', '"', '\'']));
        out.push_str(&format!(
            "  <action id=\"{id}\">\n    <description>Tetsu capability {class}</description>\n    <message>Authentication is required for {text}</message>\n    <defaults>\n      <allow_any>auth_admin</allow_any>\n      <allow_inactive>auth_admin</allow_inactive>\n      <allow_active>auth_admin</allow_active>\n    </defaults>\n  </action>\n",
            id = polkit_action_id(class),
        ));
    }
    out.push_str("</policyconfig>\n");
    Ok(out)
}
