# Tetsu — Spec iniziale

Oct 21, 2026 · @Marco Migozzi

## Visione e principi di design

Una distribuzione Linux in cui un agente AI, scelto dall'utente, gestisce l'intero sistema: aggiornamenti e loro conflitti, disco e spazio, operazioni pianificate, log e diagnosi, oltre alla produttività quotidiana (mail, messaggi, file, telefono).

**Nome di lavoro: Tetsu** (鉄, "ferro" in giapponese): richiama Rust e l'immutabilità senza dirlo, due sillabe pronunciabili uguali in ogni lingua, comando `tetsu` libero. Verificato il 21 ottobre 2026: nessuna distro, OS, compositor, agente AI o azienda con questo nome; solo un pacchetto Python interno IBM e un crate Rust sperimentale fermo alla 0.0.1. Scartati dopo verifica, tra gli altri: Aura (package manager Arch), Kairos (meta-distro CNCF), Noema, Tide, Calm (marchio Calm.com), Airo (GoDaddy), Siderite (Sidero Labs / Talos), Havn (pacchetto Arch), Kumo (NVIDIA).

Principi non negoziabili:

1. **Tutto è un tool.** Ogni funzione nasce come tool dell'agente esposto su un bus interno; la UI è solo un modo alternativo di chiamare lo stesso tool. Un plugin nuovo è usabile dall'agente il giorno stesso.
2. **Il compositor è il cuore.** Pubblica sul bus lo stato del desktop (finestra attiva, selezione, clipboard, monitor) così l'agente ha sempre il contesto di "questo file", "questa finestra".
3. **L'AI non vede mai i segreti.** Chiede operazioni, non credenziali.
4. **Permessi per capability, non per azione.** Si approva una volta "può inviare mail", poi vale per sempre; si chiede solo per operazioni distruttive.
5. **Leggero.** Niente modelli locali pesanti di default; demoni in Rust; effetti tarati sulla macchina al setup.
6. **Un solo look.** Shell, greeter, app native e decorazioni condividono palette e stile; tema GTK/Qt generato dalla stessa palette.
7. **Riscrivere il design, non i protocolli.** Smithay per Wayland, standard freedesktop (Secret Service, portals, layer-shell) dove esistono.

## Base di sistema

Immagine immutabile costruita su Arch Linux, con il parco pacchetti completo di Arch (pacman e AUR), non solo Flatpak.

- **Root in sola lettura su Btrfs**: ogni installazione o aggiornamento costruisce uno snapshot nuovo in una transazione (modello transactional-update di openSUSE / rpm-ostree di Bazzite); il boot punta all'ultimo snapshot buono, rollback atomico.
- **Livello pacchetti utente** sopra l'immagine base: "installa gimp" per l'utente è Arch normale; i conflitti AUR si risolvono in una transazione separata senza toccare il sistema in uso.
- **Flatpak** come opzione aggiuntiva, non obbligo.
- **Per l'agente** ogni transazione è un'unità su cui ragiona: "questo aggiornamento ha rotto il driver, torno indietro e ti avviso".
- Scartato NixOS: potente e riproducibile, ma linguaggio ostico ed ecosistema meno vasto di Arch.

## Sicurezza

**Disco.** LUKS2 a livello di blocco sotto Btrfs (costo trascurabile con AES-NI). Niente cifratura per singolo file (fscrypt, eCryptfs). Catena di fiducia intera: Secure Boot, Unified Kernel Image firmata, chiave LUKS legata alla TPM con systemd-cryptenroll; chiave di recupero mostrata al setup.

Tre livelli scelti al setup:

| Livello | Sblocco disco | Accesso sessione |
| --- | --- | --- |
| Comodo | TPM automatico | Password / PIN / impronta al login |
| Bilanciato | TPM + PIN | Password al login |
| Paranoico | Password completa al boot | Password al login |

Il lock screen è funzione del compositor e blocca anche l'input; richiede sempre la password.

**Segreti.** Demone separato in Rust con vault cifrato, sbloccato al login, compatibile con Secret Service di freedesktop. L'agente non riceve mai token: invoca il tool ("invia mail") e il demone esegue la chiamata o avvia il processo figlio con l'env popolato (stile systemd-creds). Il pannello nella shell è l'unica interfaccia di scrittura; ogni accesso è loggato.

**Capability.** Permessi persistenti approvati una volta per funzione, con tre modalità: automatico, notifica, chiedi (solo operazioni distruttive). Il contenuto di mail e messaggi è trattato come dato non fidato contro la prompt injection.

## Compositor e shell

Compositor e shell scritti da zero in Rust, prendendo le idee giuste da [Umbriel](https://github.com/noctalia-dev/umbriel) (C++23, wlroots: layout scrolling, dwindle e master, blur, ombre, animazioni) e da [Noctalia 5](https://noctalia.dev/) (shell in C++ e OpenGL ES senza Qt/GTK, 6 volte meno memoria, greeter con la stessa palette). Riferimenti di studio, non base da forkare.

**Compositor** (Rust, [Smithay](https://github.com/Smithay/smithay), come COSMIC e niri):

- Layout: tiling, scrolling e flottante stile macOS, scelti dall'utente, con regole per finestra.
- Decorazioni lato server imposte, così ogni app segue il tema.
- Bus eventi interno: finestra attiva, selezione, clipboard, hotplug monitor, workspace; il contratto del bus è la parte che non si potrà più cambiare.
- Hotplug monitor: evento sul bus, l'agente chiede posizione, rotazione, mirroring; la logica di Monique entra come tool di layout e l'agente la impara.
- Profilo prestazioni al setup: blur, ombre e animazioni tarati su CPU e GPU.

**Shell** (Rust, processo separato dal compositor: un crash della UI non butta giù la sessione):

- Status bar ricca, launcher universale che è anche la chat con l'agente (sessioni avviabili, stile Claude Desktop / Codex), pannello impostazioni completo, pannello segreti e capability.
- Widget e pannelli per mail, messaggi, telefono, transazioni di sistema.
- Viewer artefatti integrato (WebView leggera): Markdown, HTML, PDF, CSV, slide, fogli in sola lettura; "apri con" verso LibreOffice, OnlyOffice o Google Fogli sull'account giusto.
- Wallpaper manager con transizioni, effetti, notifiche interattive come canale dell'agente.
- Dettatura: shortcut che accende il microfono, testo al cursore.
- Greeter coerente dal boot.

**Toolkit nativo** per shell, file manager e viewer; tema GTK e Qt generato dalla palette. Coerenza al 100 % nelle app di terze parti non è raggiungibile (nemmeno macOS con Electron).

## Agente di sistema

Demone in Rust che scopre i tool sul bus (come un client MCP scopre i server) e li orchestra.

**Backend pluggabile**, scelto dall'utente, con contatore di costo visibile nella shell:

| Backend | Uso | Nota |
| --- | --- | --- |
| Abbonamento Claude Code / Codex | Sessione in background che riceve il testo della chat (SDK non interattivo) | Zona grigia nei termini di servizio: opzione, non default |
| API con chiave propria | Ragionamento, risposte, uso dei tool | Costo per token |
| Modello locale | Opzionale | Richiede GPU |

**Divisione locale / remoto.** In locale solo ciò che è affidabile e leggero: dettatura (Whisper o derivati) e classificazione a poche etichette (mail spam/newsletter/urgente, log con errore sì/no, file è fattura). Tutto ciò che richiede ragionamento va ai modelli grandi.

**Pipeline dettatura.** Whisper locale produce il testo grezzo; un passaggio di pulizia toglie riempitivi e corregge i termini con un dizionario personale che cresce con l'uso (nomi di progetti, termini tecnici). Nella chat la pulizia la fa il modello grande; in una finestra qualsiasi un modello piccolo locale. Riunioni trascritte in batch col modello Whisper grande.

**Memoria a tre livelli** (modello MemGPT / Letta): nucleo in file Markdown leggibili e modificabili dall'utente, sempre nel contesto; livello di richiamo ricercabile (Mem0 self-hosted o Graphiti per i pattern temporali, es. "archivia le newsletter il lunedì"); archivio a lungo termine. Consolidamento notturno che compatta le memorie. Cognee scartato perché troppo pesante per un desktop.

**Tool di sistema**: transazioni (aggiorna, installa, rollback), disco e spazio, log e diagnosi, operazioni pianificate, monitor, notifiche.

## Integrazioni

La distro è un host MCP, ma con un **livello aggregatore proprio** in mezzo: registro di account (provider, etichetta "personale", "lavoro", "cliente X", credenziali nel vault) e tool unificati con parametro account o "tutti". L'agente vede un solo tool `mail`, `file`, `calendar`; sotto, il demone istanzia un server per account o parla direttamente con le API. Così l'agente impara le abitudini per account (fatture sulla P.IVA, clienti sulla mail lavoro).

| Integrazione | Via | Stato |
| --- | --- | --- |
| Gmail, Drive, Calendar | [Google Workspace MCP ufficiale](https://skyvia.com/blog/best-mcp-servers/) (preview da maggio 2026) o API dirette | Fase 3 |
| Outlook / Microsoft 365 | Work IQ MCP o ms-365-mcp-server | Fase 3 |
| IMAP / SMTP generico | API dirette | Fase 3 |
| Cloud file (Drive, OneDrive, SMB) | GVFS o rclone via FUSE, montati come cartelle | Fase 3 |
| Telegram | API aperte | Fase 3, per primo |
| Telefono (Android, iOS) | Protocollo KDE Connect implementato nel demone (JSON su TLS in LAN): notifiche, file, clipboard; su iOS niente notifiche per limite Apple | Fase 3 |
| Instagram, Facebook | Meta Graph API: app registrata, review di Meta, account business/creator | Fase 4, plugin |
| X | API ufficiale a pagamento (\~100 $/mese piano base) | Fase 4, da valutare |
| WhatsApp | Nessuna API ufficiale per account personali (Business Platform solo per numeri aziendali); protocollo non ufficiale rischia il ban | Parcheggiato |
| TikTok | — | Lasciato perdere |

I social sono plugin a parte con la stessa interfaccia "pubblica, leggi notifiche". Per iOS il buco notifiche si compensa con mail, Telegram e social già integrati sul desktop: la notifica la genera l'agente. Una app mobile propria è un progetto nel progetto, rimandato.

## Strumenti di produttività

Tutti accettati; priorità da definire. Ognuno nasce come tool dell'agente.

- **File manager proprio**: non per i cloud (già montati via FUSE) ma perché ogni azione (apri, seleziona, sposta) finisce sul bus come contesto, e l'anteprima artefatti vive lì dentro. Supporta dispositivi esterni e dischi di rete.
- **Indice semantico locale dei file**: "la fattura di marzo del cliente X".
- **Launcher universale** stile Raycast in linguaggio naturale: app, file, comandi, chat.
- **Registratore riunioni**: audio di sistema trascritto con Whisper locale, appunti e azioni.
- **Note persistenti** in Markdown gestite dall'agente, sincronizzabili con Obsidian.
- **Terminale integrato** con sessioni Claude Code / Codex riprendibili; l'agente vede l'output e propone fix.
- **Clipboard manager** con storico interrogabile; clipboard condivisa col telefono attiva di default.
- **Screenshot e registrazione schermo** con estrazione testo e spiegazione dell'errore a video.
- **Timeline**: diario automatico della giornata, utile ai freelance per fatturare le ore.
- **Gestore sessioni di lavoro**: salva e ripristina finestre, progetti e contesto.
- **Riepilogo di fine giornata**: fatto, in sospeso, domani.

## Roadmap a fasi

Ogni fase produce qualcosa di usabile. Fase 0 e fase 1 vanno trattate come un unico spec Trailhead: il contratto del bus è ciò che non si potrà più cambiare.

1. **Fase 0 — Fondamenta.** Immagine immutabile su Arch con Btrfs, transazioni atomiche e rollback; LUKS2 con TPM e i tre livelli di sicurezza; compositor in Rust su Smithay con tiling, scrolling e flottante; bus eventi e protocollo tool. Nessuna AI.
2. **Fase 1 — Agente di sistema.** Demone con backend pluggabile, vault segreti, capability persistenti; tool di transazioni, disco, log, operazioni pianificate, monitor. Risultato: una distro che si auto-mantiene, usabile ogni giorno.
3. **Fase 2 — Shell.** Status bar, launcher con chat, impostazioni, viewer artefatti, dettatura con pulizia, wallpaper ed effetti, profilo prestazioni al setup, greeter coerente. Prima demo pubblica.
4. **Fase 3 — Mondo esterno.** Aggregatore multi-account per mail, calendari e cloud; Telegram; protocollo KDE Connect; memoria a tre livelli con consolidamento notturno.
5. **Fase 4 — Estensioni.** File manager, social via Graph API, plugin di terze parti, riunioni, timeline, sessioni, indice semantico.

## Fase 0 — dettaglio per Trailhead

Obiettivo della fase: un'immagine Tetsu installabile su una macchina reale che avvia un compositor Rust funzionante, si aggiorna per transazioni con rollback, e pubblica eventi su un bus la cui forma è definitiva. Nessuna AI, nessuna shell grafica oltre a una barra minima di debug.

**Deliverable**

1. `tetsu-image`: pipeline che costruisce da Arch un'immagine root in sola lettura su Btrfs (subvolume per snapshot), con UKI firmata, Secure Boot, LUKS2 e i tre livelli di sblocco (Comodo: TPM; Bilanciato: TPM + PIN; Paranoico: password). Installer testuale che chiede solo disco, livello di sicurezza, utente.
2. `tetsu-tx`: demone Rust delle transazioni. Comandi: `tx update`, `tx install <pkg>`, `tx rollback`, `tx list`. Ogni transazione crea uno snapshot, applica pacman nel nuovo snapshot, aggiorna il boot entry; il livello utente (pacman/AUR) vive in un subvolume separato dalla base.
3. `tetsu-comp`: compositor Wayland su Smithay. Tiling e flottante (scrolling in fase 2), decorazioni lato server, multi-monitor con hotplug, lock screen, xdg-shell e layer-shell. Niente blur, ombre o animazioni in questa fase.
4. `tetsu-bus`: libreria Rust condivisa + socket Unix. Definisce gli eventi e le chiamate che il compositor pubblica e che i demoni espongono. Versionato: ogni messaggio porta `v: 1`.
5. `tetsu-bar`: barra di stato minima (ora, batteria, rete, workspace) che consuma il bus. Serve solo a dimostrare che il bus funziona.

**Bozza del contratto del bus** (da chiudere nella spec Trailhead, non nel codice)

| Evento o chiamata | Direzione | Payload |
| --- | --- | --- |
| `window.focused` | compositor → bus | id, app\_id, title, workspace, output |
| `window.list` | chiamata | elenco finestre con geometria e stato |
| `workspace.changed` | compositor → bus | output, workspace |
| `output.added` / `output.removed` | compositor → bus | name, modes, edid |
| `output.configure` | chiamata | name, position, scale, rotation, mirror |
| `clipboard.changed` | compositor → bus | mime types (mai il contenuto) |
| `session.locked` / `session.unlocked` | compositor → bus | — |
| `tx.started` / `tx.finished` / `tx.failed` | tetsu-tx → bus | id, kind, packages, snapshot |
| `tx.run` | chiamata | kind, packages |
| `notify.show` | chiamata | title, body, actions |

Regole: JSON su socket Unix, un messaggio per riga; ogni demone si registra con un nome e dichiara le chiamate che espone; i tool dell'agente in fase 1 saranno una mappatura 1:1 di queste chiamate.

**Criteri di accettazione**

- Installazione da zero su un portatile reale e su una VM, con ciascuno dei tre livelli di sicurezza.
- `tx update` seguito da `tx rollback` riporta il sistema allo stato precedente; un aggiornamento interrotto a metà (spina staccata) non lascia il sistema non avviabile.
- Il compositor regge una sessione di lavoro di una giornata con Firefox, un terminale e un'app Electron senza crash; hotplug di un secondo monitor e rimozione senza perdere finestre.
- La barra riceve `window.focused` entro 50 ms dal cambio di focus.
- Il contratto del bus ha una suite di test che fallisce se un campo cambia nome o tipo.

**Decisioni già prese** (non ridiscutere in Trailhead): Arch come base, Btrfs, Smithay, Rust per tutti i demoni, JSON su socket per il bus, niente Flatpak obbligatorio, niente AI in questa fase.

**Non-obiettivi della fase 0**: shell grafica, launcher, impostazioni, temi, effetti, dettatura, qualsiasi integrazione esterna, file manager, greeter personalizzato (si usa greetd).

**Domande da far risolvere a Trailhead**: toolkit per la barra (iced, egui, o drawing diretto); strumento di build dell'immagine (mkosi, archiso modificato, script propri); formato dei boot entry (systemd-boot con UKI è l'ipotesi); come esporre il livello utente pacman senza rompere l'atomicità della base.

## Decisioni aperte e rischi

- [ ] Licenza (Smithay è MIT, niri GPLv3: la scelta condiziona cosa si può studiare e riusare); verifica marchio EUIPO/USPTO e domini per "Tetsu" prima della prima release pubblica.
- [ ] Toolkit UI nativo in Rust per la shell (iced come COSMIC, o altro).
- [ ] Formato del contratto del bus e del protocollo tool (MCP nativo, D-Bus, o protocollo proprio su socket).
- [ ] Priorità tra gli strumenti di produttività.
- [ ] Modello Whisper per l'italiano: quale taglia per dettatura breve e per riunioni.
- [ ] Quando aggiungere X e una app mobile propria.

Rischi principali:

- Termini di servizio degli abbonamenti Claude Code / Codex usati come backend: possono cambiare.
- Prompt injection da mail, messaggi e pagine web: il livello capability deve trattarli come dati non fidati.
- Meta e X possono restringere le API o la review dell'app.
- Umbriel e il modello transazionale su Arch sono giovani: rough edges attesi.
- Scope: compositor, shell, agente e file manager da zero sono quattro progetti; il bus è l'unico che non può slittare.
