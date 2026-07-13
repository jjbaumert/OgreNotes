# OgreNotes — German (de) translations.
#
# Source-of-truth catalog is locales/en-US/main.ftl. Keys present
# in en-US but missing here fall back to en-US at runtime.

# ─── Common ─────────────────────────────────────────────────────

common-loading = Wird geladen…
common-send = Senden
common-close = Schließen
# Document Details panel (#141)
document-details-title = Dokumentdetails
document-details-name = Name
document-details-type = Typ
document-details-created = Erstellt
document-details-modified = Zuletzt geändert
document-details-words = Wörter
document-details-characters = Zeichen
doc-type-document = Dokument
doc-type-spreadsheet = Tabelle
# Editor gutter (#139)
editor-page-break = Seite { $n }
# Find & Replace bar (#147)
find-placeholder = Suchen
find-replace-placeholder = Ersetzen durch
find-no-results = Keine Ergebnisse
find-prev = Vorheriger Treffer
find-next = Nächster Treffer
find-replace = Ersetzen
find-replace-all = Alle ersetzen
common-delete = Löschen
common-cancel = Abbrechen
common-untitled = Ohne Titel
common-redirecting-login = Weiterleitung zur Anmeldung…
common-redirecting = Weiterleitung…
common-open-navigation = Navigation öffnen
common-restore-here = Hier wiederherstellen

# ─── Sidebar ────────────────────────────────────────────────────

sidebar-section-navigation = Navigation
sidebar-section-recent = Zuletzt
sidebar-section-pinned = Angeheftet
sidebar-empty-recent = Keine zuletzt verwendeten Einträge
sidebar-empty-pinned = Keine angehefteten Einträge
# Favorites (#144)
document-favorite = Zu Favoriten hinzufügen
# Expand / full screen (#145)
document-expand-enter = Auf Vollbild erweitern
document-expand-exit = Vollbild beenden
document-unfavorite = Aus Favoriten entfernen
sidebar-section-favorites = Favoriten
sidebar-empty-favorites = Noch keine Favoriten
sidebar-home = Start
sidebar-search = Suche
sidebar-sign-out = Abmelden
sidebar-aria-main-nav = Hauptnavigation
sidebar-aria-collapse = Seitenleiste einklappen
sidebar-aria-expand = Seitenleiste ausklappen

# ─── Menu bar ───────────────────────────────────────────────────

menubar-document = Dokument
menubar-edit = Bearbeiten
menubar-view = Ansicht
menubar-insert = Einfügen
menubar-format = Format

# ─── Notification panel ─────────────────────────────────────────

notifications-title = Benachrichtigungen
notifications-mark-all-read = Alle als gelesen markieren
notifications-empty = Keine Benachrichtigungen

# ─── Chat panel ─────────────────────────────────────────────────

chat-section-title = Chats
chat-empty = Noch keine Chats
chat-back = ← Zurück zu den Chats
chat-new = + Neuer Chat
chat-message-placeholder = Nachricht eingeben…

# ─── Document outline ───────────────────────────────────────────

outline-title = Gliederung
outline-empty = Keine Überschriften
outline-aria-close = Gliederung schließen

# ─── Editor toolbar ─────────────────────────────────────────────

toolbar-undo = Rückgängig (Strg+Z)
toolbar-redo = Wiederherstellen (Strg+Umschalt+Z)
toolbar-bold = Fett (Strg+B)
toolbar-italic = Kursiv (Strg+I)
toolbar-underline = Unterstreichen (Strg+U)
toolbar-strikethrough = Durchgestrichen
# Subscript / Superscript (#143)
toolbar-subscript = Tiefgestellt
toolbar-superscript = Hochgestellt
toolbar-code = Code (Strg+E)
# Toolbar alignment controls (#134)
toolbar-align-left = Linksbündig
toolbar-align-center = Zentriert
toolbar-align-right = Rechtsbündig
toolbar-text-color = Textfarbe
toolbar-remove-color = Farbe entfernen
toolbar-highlight = Hervorheben
toolbar-remove-highlight = Hervorhebung entfernen
toolbar-image = Bild
toolbar-link = Link (Strg+K)
toolbar-horizontal-rule = Horizontale Linie
toolbar-insert-table = Tabelle einfügen
toolbar-insert-label = Einfügen:
toolbar-comment = Kommentieren (Strg+Alt+C)
toolbar-comment-label = Kommentieren
toolbar-more = Mehr
toolbar-aria-more = Weitere Symbolleisten-Optionen
toolbar-prompt-url = URL eingeben:

# Block-type dropdown
toolbar-block-paragraph = Absatz
toolbar-block-heading-1 = Überschrift 1
toolbar-block-heading-2 = Überschrift 2
toolbar-block-heading-3 = Überschrift 3
toolbar-block-heading-4 = Überschrift 4
toolbar-block-bulleted-list = Aufzählungsliste
toolbar-block-numbered-list = Nummerierte Liste
toolbar-block-checklist = Checkliste
toolbar-block-blockquote = Zitat
toolbar-block-code-block = Codeblock
toolbar-block-format = Format

# Number formats (spreadsheet block-type menu)
toolbar-num-general = Allgemein
toolbar-num-integer = Ganzzahl
toolbar-num-decimal-1 = Dezimal (1)
toolbar-num-decimal-2 = Dezimal (2)
toolbar-num-thousands = Tausender
toolbar-num-currency-usd = Währung (USD)
toolbar-num-currency-eur = Währung (EUR)
toolbar-num-percent = Prozent

# ─── Comment popup ──────────────────────────────────────────────

comment-new-title = Neuer Kommentar
comment-thread-title = Kommentar-Thread
comment-aria-prev = Vorheriger Kommentar
comment-aria-next = Nächster Kommentar
comment-placeholder-new = Einen Kommentar zu diesem Abschnitt hinzufügen
comment-placeholder-reply = Nachricht eingeben…

# ─── Conversation pane ──────────────────────────────────────────

conversation-thread = Thread
conversation-comment-on-block = Block kommentieren
conversation-comments = Kommentare
conversation-empty = Noch keine Kommentare. Starte eine Unterhaltung!
conversation-placeholder-block = Diesen Block kommentieren…
conversation-placeholder-add = Kommentar hinzufügen…
conversation-placeholder-reply = Antworten…
conversation-aria-prev = Vorheriger Kommentar
conversation-aria-next = Nächster Kommentar
conversation-back = ← Zurück
conversation-status-open = Offen
conversation-status-resolved = Erledigt
conversation-resolve = Erledigen
conversation-reopen = Erneut öffnen
conversation-typing-1 = { $name } tippt…
conversation-typing-2 = { $a } und { $b } tippen…
conversation-typing-many = Mehrere Personen tippen…

# ─── History viewer ─────────────────────────────────────────────

history-title = Bearbeitungsverlauf
history-empty = Noch kein Versionsverlauf
history-no-prior = Keine frühere Version zum Vergleich — dies ist der erste Schnappschuss.
history-changes-in-v = Änderungen in v{ $version }
history-restore-version = Version wiederherstellen
history-aria-close = Schließen
history-jump-to-block-title = Zum Block im Live-Dokument springen
history-jump-to-block-label = Zum Block springen ↗
history-restoring = Wiederherstellen…
history-restore-to-this-version = Auf diese Version zurücksetzen
history-restore-confirm-message = Das aktuelle Dokument durch diese Version ersetzen? Nicht gespeicherte lokale Änderungen gehen verloren.
history-restore-confirm-label = Wiederherstellen
history-deleted-badge = (gelöscht)

# Node-type labels for diff cards
node-paragraph = Absatz
node-heading = Überschrift
node-bullet-list = Aufzählungsliste
node-ordered-list = Nummerierte Liste
node-list-item = Listeneintrag
node-task-list = Aufgabenliste
node-task-item = Aufgabe
node-blockquote = Zitat
node-code-block = Codeblock
node-horizontal-rule = Trenner
node-image = Bild
node-table = Tabelle
node-table-row = Tabellenzeile
node-table-cell = Tabellenzelle
node-table-header = Tabellenüberschrift
node-block = Block

# ─── Login page ─────────────────────────────────────────────────

login-tagline = Dokumente mit Biss.
login-error-name-email-required = Name und E-Mail sind erforderlich
login-placeholder-name = Anzeigename
login-placeholder-email = E-Mail
login-signing-in = Anmeldung läuft…
login-dev-button = Entwickler-Login (benutzerdefiniert)
login-github = Mit GitHub anmelden
login-google = Mit Google anmelden

# ─── Share dialog ───────────────────────────────────────────────

share-title = Teilen
share-placeholder-email = E-Mail-Adresse eingeben
share-button = Teilen
share-members-heading = Aktuelle Mitglieder
share-role-owner = Eigentümer
share-role-edit = Kann bearbeiten
share-role-comment = Kann kommentieren
share-role-view = Kann ansehen
share-error-no-folder = Dokument hat keinen Ordner — Teilen nicht möglich
share-error-enter-email = E-Mail-Adresse eingeben
share-status-searching = Suche läuft…
share-error-search-failed = Suche fehlgeschlagen: { $err }
share-error-no-user = Kein Benutzer mit der E-Mail „{ $email }“ gefunden
share-status-shared-with = Geteilt mit { $name }
share-error-failed = Teilen fehlgeschlagen: { $err }

# Linkfreigabe (dokumentbezogener Abschnitt des Teilen-Dialogs)
share-link-heading = Linkfreigabe
share-link-mode-off = Aus
share-link-mode-view = Kann ansehen
share-link-mode-edit = Kann bearbeiten
share-link-note = Jeder in Ihrem Workspace mit dem Link kann dies öffnen.
share-link-off = Linkfreigabe ist deaktiviert.
share-link-opt-comments = Kommentare zulassen
share-link-opt-history = Bearbeitungsverlauf anzeigen
share-link-opt-conversation = Konversation anzeigen
share-link-opt-request = Bearbeitungsanfragen zulassen
share-link-copy = Link kopieren
share-link-copied = Link kopiert
share-link-saved = Gespeichert
share-link-error = Speichern fehlgeschlagen: { $err }
# Viewer-facing request-edit-access affordance (#110)
share-link-request-banner = Sie sehen dieses Dokument über einen geteilten Link.
share-link-request-button = Bearbeitungszugriff anfordern
share-link-request-sending = Wird gesendet…
share-link-request-sent = Anfrage gesendet
share-link-request-retry = Senden fehlgeschlagen – erneut versuchen

# ─── Document page ──────────────────────────────────────────────

document-loading = Dokument wird geladen…
document-trash-banner = Dieses Dokument befindet sich im Papierkorb — zum Bearbeiten wiederherstellen.
document-trash-restore = Wiederherstellen
document-trash-delete-forever = Endgültig löschen
document-share-tooltip = Teilen
# Document menu: rename + move (#146)
document-rename-prompt = Dokument umbenennen
document-move-folder-title = In Ordner verschieben
document-move-here = Hierher verschieben
# Duplicate dialog (#146)
duplicate-dialog-title = Dokument duplizieren
duplicate-name-label = Name
duplicate-destination-label = Zielordner
duplicate-confirm = Duplizieren
duplicate-share-warning = Dieser Ordner ist freigegeben — { $count } weitere Personen haben Zugriff und sehen somit die Kopie.
# Focus/expand toggle (#134)
document-focus-enter = Fokusmodus
document-focus-exit = Fokusmodus beenden
document-trash-dialog-title = In den Papierkorb verschieben
document-trash-dialog-message = Dieses Dokument wird in den Papierkorb verschoben. Du kannst es später wiederherstellen.
document-trash-dialog-confirm = In den Papierkorb verschieben
document-purge-dialog-title = Endgültig löschen?
document-purge-dialog-message = Dadurch wird das Dokument samt allen Inhalten dauerhaft gelöscht. Dies kann nicht rückgängig gemacht werden.
document-purge-dialog-confirm = Endgültig löschen
document-restore-folder-title = In Ordner wiederherstellen

# ─── Home page ──────────────────────────────────────────────────

home-new-document = + Neues Dokument
home-new-spreadsheet = + Neue Tabelle
home-new-folder = + Neuer Ordner

# ─── MFA (enroll + challenge) ───────────────────────────────────

mfa-verifying = Überprüfung…
mfa-enter-totp = Gib den 6-stelligen Code aus deiner Authenticator-App ein
mfa-enter-recovery = Gib deinen Wiederherstellungscode ein

mfa-enroll-title = Zwei-Faktor-Authentifizierung einrichten
mfa-enroll-subtitle = Scanne den QR-Code mit deiner Authenticator-App und gib dann den 6-stelligen Code zur Bestätigung ein.
mfa-enroll-success = Einrichtung bestätigt. Weiterleitung…
mfa-enroll-error-failed = Einrichtung fehlgeschlagen: { $err }
mfa-enroll-error-verify-failed = Überprüfung fehlgeschlagen: { $err }
mfa-enroll-manual-entry = Manuelle Eingabe
mfa-enroll-recovery-codes-summary = Wiederherstellungscodes (jetzt speichern!)
mfa-enroll-recovery-warning = Jeder Code kann einmal verwendet werden, falls du den Zugriff auf deinen Authenticator verlierst. Wir zeigen sie nicht erneut an.
mfa-enroll-code-label = Authenticator-Code
mfa-enroll-confirm = Bestätigen

mfa-challenge-title = Zwei-Faktor-Authentifizierung
mfa-challenge-subtitle-totp = Öffne deine Authenticator-App und gib den 6-stelligen Code ein.
mfa-challenge-subtitle-recovery = Gib einen deiner einmaligen Wiederherstellungscodes ein.
mfa-challenge-verify = Überprüfen
mfa-challenge-missing-handle = MFA-Handle fehlt, Weiterleitung zur Anmeldung…
mfa-challenge-error-invalid-totp = Ungültiger Code — prüfe deinen Authenticator und versuche es erneut
mfa-challenge-error-invalid-recovery = Ungültiger Wiederherstellungscode — jeder Code kann nur einmal verwendet werden
mfa-challenge-use-totp = Stattdessen Authenticator-Code verwenden
mfa-challenge-use-recovery = Authenticator verloren? Wiederherstellungscode verwenden

# ─── Admin console (platform-admin pages) ───────────────────────

admin-loading = Admin-Konsole wird geladen…
admin-redirecting = Weiterleitung…
admin-status-active = aktiv
admin-status-disabled = deaktiviert
admin-status-never = nie
admin-role-admin = Administrator
admin-role-user = Benutzer
admin-retry = Erneut versuchen

# Admin sub-nav
admin-nav-users = Benutzer
admin-nav-metrics = Kennzahlen
admin-nav-audit = Audit
admin-nav-back = Zurück zur App

# Admin · Users
admin-users-title = Admin · Benutzer
admin-users-search-placeholder = Nach E-Mail-Präfix filtern
admin-users-th-email = E-Mail
admin-users-th-name = Name
admin-users-th-role = Rolle
admin-users-th-state = Status
admin-users-th-last-active = Zuletzt aktiv
admin-users-th-actions = Aktionen
admin-users-enable = Aktivieren
admin-users-disable = Deaktivieren
admin-users-promote = Heraufstufen
admin-users-demote = Herabstufen
admin-users-prev = Zurück
admin-users-next = Weiter
admin-users-error-list-failed = Auflisten fehlgeschlagen: { $err }
admin-users-error-action-failed = { $action } fehlgeschlagen: { $err }

# Admin · Audit
admin-audit-title = Admin · Audit-Protokoll
admin-audit-label-target = Ziel-Benutzer-ID
admin-audit-label-actor = Akteur-Benutzer-ID
admin-audit-label-kind = Art
admin-audit-placeholder-kind = z. B. disable, loginFailure
admin-audit-label-from = Von (ISO)
admin-audit-label-to = Bis (ISO)
admin-audit-search = Suchen
admin-audit-error-target-required = Ziel-Benutzer-ID ist erforderlich
admin-audit-error-load-failed = Laden fehlgeschlagen: { $err }
admin-audit-th-when = Wann
admin-audit-th-source = Quelle
admin-audit-th-kind = Art
admin-audit-th-actor = Akteur
admin-audit-th-target = Ziel
admin-audit-th-detail = Details

# Admin · Metrics
admin-metrics-title = Admin · Kennzahlen
admin-metrics-refresh = Aktualisieren
admin-metrics-error-fetch-failed = Abrufen fehlgeschlagen: { $err }
admin-metrics-counters = Zähler
admin-metrics-gauges = Messwerte
admin-metrics-histograms = Histogramme
admin-metrics-th-key = Schlüssel
admin-metrics-th-value = Wert
admin-metrics-th-count = Anzahl
admin-metrics-th-sum = Summe
admin-metrics-th-min = Min
admin-metrics-th-max = Max

# ─── Workspace SCIM tokens ──────────────────────────────────────

scim-title = SCIM-Tokens des Arbeitsbereichs
scim-subtitle = Erstelle ein Bearer-Token für den SCIM-Provisioning-Connector deines IdP. Der Klartext wird nur einmal beim Erstellen angezeigt — kopiere ihn sofort.
scim-base-url-heading = SCIM-Basis-URL
scim-base-url-help = Füge dies in die Konfiguration des SCIM-Connectors deines IdP ein.
scim-fresh-heading = Neues Token: { $name }
scim-fresh-warning = Kopiere dieses Token JETZT — es wird nicht erneut angezeigt.
scim-fresh-copy = Kopieren
scim-create-heading = Neues Token erstellen
scim-create-placeholder = Bezeichnung (z. B. Okta-Connector)
scim-create-button = Erstellen
scim-existing-heading = Vorhandene Tokens
scim-empty = Noch keine Tokens. Erstelle oben eines.
scim-th-name = Name
scim-th-token-id = Token-ID
scim-th-created = Erstellt
scim-th-last-used = Zuletzt verwendet
scim-th-status = Status
scim-status-active = aktiv
scim-status-revoked = widerrufen
scim-revoke = Widerrufen
scim-error-name-required = Token-Name ist erforderlich
scim-error-load-failed = Laden fehlgeschlagen: { $err }
scim-error-create-failed = Erstellen fehlgeschlagen: { $err }
scim-error-revoke-failed = Widerrufen fehlgeschlagen: { $err }

# ─── Workspace SAML SSO ─────────────────────────────────────────

saml-title = SAML-SSO des Arbeitsbereichs
saml-subtitle-prefix = Konfiguriere einen SAML-2.0-IdP für diesen Arbeitsbereich. Mitglieder können sich über den IdP anmelden unter
saml-subtitle-suffix = sobald du gespeichert hast.
saml-status-saved = SAML-Konfiguration gespeichert.
saml-status-removed = SAML-Konfiguration entfernt.
saml-status-copied = SP-Metadaten-URL in die Zwischenablage kopiert.
saml-sp-heading = SP-Metadaten
saml-sp-help = Kopiere diese URL in den Ablauf „Service Provider hinzufügen“ deines IdP. Oder rufe die URL einmal ab und füge das Antwort-XML in deinem IdP ein.
saml-copy = Kopieren
saml-idp-heading = IdP-Konfiguration
saml-idp-help = Füge das Metadaten-XML ein, das dir dein IdP gibt. Das vollständige XML ist erforderlich — einschließlich des Wurzelelements <EntityDescriptor>.
saml-label-entity-id = IdP-Entity-ID
saml-placeholder-entity-id = https://idp.example.com/metadata
saml-label-metadata-xml = IdP-Metadaten-XML
saml-label-email-attr = Name des E-Mail-Attributs
saml-label-name-attr = Name des Namen-Attributs
saml-save = Speichern
saml-update = Aktualisieren
saml-remove = Entfernen
saml-error-entity-id-required = IdP-Entity-ID ist erforderlich
saml-error-metadata-required = IdP-Metadaten-XML ist erforderlich
saml-error-load-failed = Laden fehlgeschlagen: { $err }
saml-error-save-failed = Speichern fehlgeschlagen: { $err }
saml-error-delete-failed = Löschen fehlgeschlagen: { $err }
saml-meta-first-configured = Erstmals konfiguriert
saml-meta-last-updated = ; zuletzt aktualisiert

# ─── Spreadsheet view (chrome) ──────────────────────────────────

ss-empty = Keine Daten
ss-format-painter-title = Format übertragen — klicke, um die Formatierung der aktiven Zelle zu kopieren, und klicke dann auf ein Ziel. Umschalt+Klick für den dauerhaften Modus.
ss-format-painter-status = Format übertragen — klicke auf eine Zelle zum Übernehmen, Esc zum Abbrechen
ss-format-painter-status-sticky = Format übertragen (dauerhaft) — klicke auf Zellen zum Übernehmen, Esc zum Beenden
ss-sort-tooltip = Tabelle sortieren…

# Status bar
ss-status-count = Anzahl: { $value }
ss-status-sum = Summe: { $value }
ss-status-avg = Ø: { $value }
ss-status-min = Min: { $value }
ss-status-max = Max: { $value }

# Sheet tabs
ss-rename-sheet-prompt = Blatt umbenennen:
ss-ctx-rename = Umbenennen
ss-ctx-delete = Löschen

# Find / replace bar
ss-find-placeholder = Suchen…
ss-replace-placeholder = Ersetzen…
ss-find-next = Weiter
ss-find-replace = Ersetzen
ss-find-replace-all = Alle ersetzen
ss-find-no-results = 0 Treffer

# Filter dropdown
ss-filter-header = Filter: { $col }
ss-filter-show-all = Alle anzeigen
ss-filter-custom-prompt = Benutzerdefinierter Filter (z. B. >100, <0, =Done, contains:err, empty, notempty):
ss-filter-custom-button = Benutzerdefinierter Filter…
ss-filter-empty-value = (leer)

# Sort dialog
ss-sort-title = Sortieren
ss-sort-range-label = Bereich:
ss-sort-has-headers = Erste Zeile enthält Überschriften (beim Sortieren überspringen)
ss-sort-by-label = Sortieren nach
ss-sort-then-by-label = Dann nach
ss-sort-asc = Aufsteigend
ss-sort-desc = Absteigend
ss-sort-remove-level-title = Diese Sortierebene entfernen
ss-sort-add-level = + Sortierebene hinzufügen
ss-sort-cancel = Abbrechen
ss-sort-apply = Anwenden
ss-sort-err-parse-range = Bereich konnte nicht ausgewertet werden. Verwende die A1-Notation, z. B. A1:G41.
ss-sort-err-no-keys = Mindestens einen Sortierschlüssel hinzufügen.

# Foreign-document consent dialog
ss-foreign-title = Dieses Dokument ruft Daten aus anderen Arbeitsmappen ab
ss-foreign-hint = Das Erlauben von Abrufen nutzt deinen Lesezugriff auf diese Dokumente. Die Genehmigung gilt nur für diese Sitzung.
ss-foreign-deny = Verweigern
ss-foreign-allow = Erlauben

# ─── Spreadsheet context menu (cell right-click) ────────────────

ss-ctx-menu-insert = Einfügen
ss-ctx-menu-delete = Löschen
ss-ctx-menu-sort = Sortieren
ss-ctx-menu-format = Format
ss-ctx-menu-comment = Kommentar
ss-ctx-menu-hide = Aus-/Einblenden
ss-ctx-menu-data = Daten
ss-ctx-menu-cond-fmt = Bedingte Formatierung
ss-ctx-menu-validation = Datenüberprüfung
ss-ctx-insert-row-above = Zeile darüber einfügen
ss-ctx-insert-row-below = Zeile darunter einfügen
ss-ctx-insert-col-left = Spalte links einfügen
ss-ctx-insert-col-right = Spalte rechts einfügen
ss-ctx-sort-dialog = Sortieren…
ss-ctx-delete-row = Zeile löschen
ss-ctx-delete-rows = { $count } Zeilen löschen
ss-ctx-delete-col = Spalte löschen
ss-ctx-delete-cols = { $count } Spalten löschen
ss-ctx-clear-contents = Inhalte löschen
ss-ctx-sort-a-z = Sortieren A → Z
ss-ctx-sort-z-a = Sortieren Z → A
ss-ctx-freeze-rows = Zeilen darüber fixieren
ss-ctx-unfreeze-rows = Zeilenfixierung aufheben
ss-ctx-freeze-cols = Spalten links fixieren
ss-ctx-unfreeze-cols = Spaltenfixierung aufheben

# Cell validation
ss-ctx-set-checkbox = Als Kontrollkästchen festlegen
ss-ctx-set-dropdown = Als Dropdown festlegen…
ss-ctx-remove-validation = Validierung entfernen
ss-ctx-dropdown-prompt = Dropdown-Optionen eingeben (durch Komma getrennt):

# Conditional formatting
ss-ctx-cond-fmt = Bedingte Formatierung…
ss-ctx-cond-fmt-prompt = Bedingte Formatierung (z. B. >100, <0, =Done, contains:error, empty, notempty):
ss-ctx-cond-fmt-color-prompt = Hintergrundfarbe (z. B. #ff0000, red, #ffd):
ss-ctx-color-scale = Farbskala…
ss-ctx-color-scale-prompt = Farbskala: niedrig,hoch oder niedrig,mittel,hoch (z. B. #ff0000,#ffff00,#00ff00):
ss-ctx-data-bar = Datenbalken…
ss-ctx-data-bar-prompt = Farbe des Datenbalkens:
ss-ctx-icon-set = Symbolsatz…
ss-ctx-icon-set-prompt = Symbolsatz: arrows oder traffic

# Charts + pivots
ss-ctx-insert-chart = Diagramm einfügen…
ss-ctx-chart-type-prompt = Diagrammtyp (bar, line, pie):
ss-ctx-chart-title-prompt = Diagrammtitel:
ss-ctx-chart-unknown-type = Unbekannter Diagrammtyp. Verwende eines von: bar, line, pie.
ss-ctx-insert-pivot = Pivot-Tabelle einfügen…
ss-ctx-pivot-needs-multi = Eine Pivot-Tabelle benötigt eine Auswahl über mehrere Zeilen und Spalten. Wähle deine Daten (mit der Überschriftenzeile in Zeile 1) und versuche es erneut.

# CSV import + merge
ss-ctx-import-csv = CSV importieren…
ss-ctx-merge-cells = Zellen verbinden
ss-ctx-unmerge-cells = Zellverbindung aufheben

# Hide / unhide
ss-ctx-hide-row = Zeile ausblenden
ss-ctx-unhide-all-rows = Alle Zeilen einblenden
ss-ctx-hide-col = Spalte ausblenden
ss-ctx-unhide-all-cols = Alle Spalten einblenden

# Cell lock + comments + named ranges
ss-ctx-lock-cell = Zelle sperren
ss-ctx-unlock-cell = Zelle entsperren
ss-ctx-add-comment = Kommentar hinzufügen…
ss-ctx-edit-comment = Kommentar bearbeiten…
ss-ctx-open-comment = Kommentar-Thread öffnen…
ss-ctx-comment-prompt = Kommentar:
ss-ctx-remove-comment = Kommentar entfernen
ss-comment-preview-empty = Noch keine Nachrichten
ss-comment-replies-none = Keine Antworten
ss-comment-replies-one = 1 Antwort
ss-comment-replies-many = { $count } Antworten
ss-ctx-define-name = Namen definieren…
ss-ctx-name-prompt = Name für diesen Bereich:
ss-ctx-remove-name = Namen entfernen…
ss-ctx-no-named-ranges = Keine benannten Bereiche definiert.
ss-ctx-remove-name-prompt = Welchen Namen entfernen? Definiert: { $names }

# ─── Pivot table editor ─────────────────────────────────────────

ss-pivot-title = Pivot-Tabellen-Editor
ss-pivot-foreign-source-label = Externe Quelle:
ss-pivot-foreign-hint = Das Bearbeiten externer Quellen wird noch nicht unterstützt. Bearbeite die Pivot-Konfiguration über das JSON-Attribut oder entferne sie und erstelle sie als lokale Pivot-Tabelle neu.
ss-pivot-layout = Layout
ss-pivot-layout-compact = Kompakt
ss-pivot-layout-outline = Gliederung
ss-pivot-layout-tabular = Tabellarisch
ss-pivot-totals = Summen
ss-pivot-totals-none = Keine
ss-pivot-totals-rows = Zeilen
ss-pivot-totals-cols = Spalten
ss-pivot-totals-both = Beide
ss-pivot-edit-filter-tooltip = Filter bearbeiten
ss-pivot-axis-row = Zeile
ss-pivot-axis-col = Spalte
ss-pivot-labels-header = { $axis }-Beschriftungen — { $col }
ss-pivot-close-tooltip = Schließen
ss-pivot-close-editor-tooltip = Editor schließen (Pivot bleibt sichtbar)
ss-pivot-filter-all = Alle
ss-pivot-filter-none = Keine
ss-pivot-filter-prefix = { $col } filtern
ss-pivot-source-label = Quelle:
ss-pivot-delete = Pivot löschen
ss-pivot-section-fields = Felder
ss-pivot-section-rows = Zeilen
ss-pivot-section-cols = Spalten
ss-pivot-section-values = Werte
ss-pivot-section-filters = Filter
ss-pivot-search-placeholder = Felder suchen…
ss-pivot-bin-width-tooltip = Klassenbreite

# Date granularity options
ss-pivot-date-year = Jahr
ss-pivot-date-quarter = Quartal
ss-pivot-date-month = Monat
ss-pivot-date-day = Tag
ss-pivot-date-hour = Stunde

# ─── App-level / router ─────────────────────────────────────────

app-not-found = Seite nicht gefunden

# ─── Accessibility ──────────────────────────────────────────────

a11y-skip-to-content = Zum Hauptinhalt springen
a11y-toolbar-label = Dokument-Formatierungsleiste
a11y-toolbar-group-undo = Rückgängig und Wiederherstellen
a11y-toolbar-group-block-type = Blocktyp
a11y-toolbar-group-inline = Inline-Formatierung
a11y-toolbar-group-align = Ausrichtung
a11y-toolbar-group-block = Blockformatierung
a11y-toolbar-group-insert = Einfügen
a11y-file-table-label = Dokumente und Ordner
a11y-breadcrumb-label = Ordner-Pfad

# ─── @-menu (mention picker) ────────────────────────────────────

at-menu-empty = Tippe, um Personen und Dokumente zu suchen…

# ─── File browser (home page table) ─────────────────────────────

file-browser-empty = Hier ist noch nichts. Erstelle ein Dokument oder einen Ordner.
file-browser-th-title = Titel
file-browser-th-added = Hinzugefügt
file-type-folder = Ordner
file-type-document = Dokument
file-type-spreadsheet = Tabelle
file-type-chat = Chat

# ─── Folder picker ──────────────────────────────────────────────

folder-picker-not-available =  (nicht verfügbar)

# ─── Formula keyboard ───────────────────────────────────────────

formula-key-backspace = Rücktaste
formula-key-cancel = Abbrechen (Esc)
formula-key-commit = Bestätigen (Eingabe)
# Mode-switcher tabs (Phase 5 M-P3 piece C).
kbd-mode-standard = Aa
kbd-mode-numeric = 123
kbd-mode-formula = ƒx
kbd-standard-hint = Verwende die Tastatur deines Geräts

# ─── Search dialog ──────────────────────────────────────────────

search-placeholder = Dokumente suchen…
search-searching = Suche läuft…
search-no-results = Keine Treffer gefunden
search-dialog-label = Dokumente suchen oder Befehle ausführen

# ─── Ask (RAG-Assistent) ────────────────────────────────────────

ask-dialog-title = Assistent fragen
ask-badge = KI
ask-placeholder = Stelle eine Frage zu deinen Dokumenten…
ask-empty-hint = Der Assistent durchsucht deine Dokumente und zitiert, was er findet. Stelle eine konkrete Frage.
ask-sources-heading = Quellen
ask-error-rate-limit = Zu viele Anfragen. Warte einen Moment und versuche es erneut.
ask-error-disabled = Ein Administrator hat den Assistenten für deinen Arbeitsbereich deaktiviert.
ask-error-unavailable = Der Assistent ist vorübergehend nicht verfügbar.
sidebar-ask = Fragen

# ─── Relationships ──────────────────────────────────────────────

relationship-heading = Verwandt
relationship-empty = Noch keine verwandten Dokumente.
relationship-add-aria = Verwandtes Dokument hinzufügen
relationship-remove-aria = Diese Beziehung entfernen
relationship-picker-placeholder = Dokumente zum Verknüpfen suchen…
relationship-picker-aria = Dokumente zum Verknüpfen suchen
relationship-picker-confirm = Verknüpfen
relationship-type-aria = Beziehungstyp
relationship-error-self = Ein Dokument kann nicht mit sich selbst verknüpft werden.
relation-type-implements = Implementiert
relation-type-derived-from = Abgeleitet von
relation-type-depends-on = Abhängig von
relation-type-references = Verweist auf
relation-type-supersedes = Ersetzt

# ─── Theme selector ─────────────────────────────────────────────

theme-aria-label = Design
theme-system = Systemdesign folgen
theme-light = Helles Design
theme-dark = Dunkles Design

# ─── Locale selector ────────────────────────────────────────────

locale-aria-label = Sprache

# ─── Inline selection / comment-bubble ──────────────────────────

selection-toolbar-comment = Auswahl kommentieren
comment-highlights-add = Kommentar hinzufügen

# ─── Auth-callback page ─────────────────────────────────────────

auth-complete-status = Anmeldung wird abgeschlossen…

# ─── Sync indicator (Phase 5 M-P3 piece B) ──────────────────────

sync-saved = Gespeichert
sync-saving = Speichern…
sync-offline = Offline
sync-offline-pending = Offline — {$count} ausstehend
sync-saved-tooltip = Deine Änderungen sind gespeichert.
sync-saving-tooltip = Deine letzten Änderungen werden an den Server gesendet…
sync-offline-tooltip = Du bist offline. Verbinde dich erneut, um weiter zusammenzuarbeiten.
sync-offline-pending-tooltip = Du bist offline. {$count} Änderung(en) haben den Server noch nicht erreicht.

# ─── Command palette (Phase 5 M-P4 piece A) ─────────────────────

palette-no-actions = Keine passenden Befehle.
cmd-go-home = Zur Startseite
cmd-toggle-dark-mode = Dunkelmodus umschalten
cmd-open-trash = Papierkorb öffnen
cmd-sign-out = Abmelden
cmd-ask = Assistent fragen
cmd-about-palette = Befehlspalette: Info
# Editor-scoped commands (M-P4 piece B).
cmd-bold = Fett
cmd-italic = Kursiv
cmd-underline = Unterstreichen
cmd-strike = Durchgestrichen
cmd-code = Inline-Code
cmd-heading-1 = Überschrift 1
cmd-heading-2 = Überschrift 2
cmd-heading-3 = Überschrift 3
cmd-paragraph = Absatz
cmd-bullet-list = Aufzählungsliste
cmd-ordered-list = Nummerierte Liste
cmd-task-list = Aufgabenliste
cmd-blockquote = Zitat
cmd-code-block = Codeblock
cmd-divider = Trenner einfügen
cmd-insert-table = Tabelle einfügen
cmd-undo = Rückgängig
cmd-redo = Wiederherstellen

# ─── Home-page drop-to-import (Phase 5 M-P5 piece D) ────────────

home-drop-title = Zum Importieren ablegen
home-drop-hint = Markdown (.md) oder HTML (.html) — bis zu 1 MB
home-import-default-title = Importiert

# ─── Toolbar — embed insert (Phase 5 M-P6 piece B) ──────────────

toolbar-embed = Medien einbetten (URL)

# ─── File-browser bulk selection (Phase 5 M-P7 piece C) ─────────

file-browser-th-select = Auswählen
bulk-selection-count = {$count} ausgewählt
bulk-selection-cancel = Abbrechen
bulk-selection-delete = Löschen
bulk-delete-dialog-title = Ausgewählte Dokumente in den Papierkorb verschieben?
bulk-delete-dialog-message = Die ausgewählten Dokumente werden in deinen Papierkorb verschoben. Du kannst sie innerhalb von 30 Tagen wiederherstellen.
bulk-delete-dialog-confirm = In den Papierkorb verschieben

# ─── Account settings page (design/account-menu.md, step 1) ─────

settings-title = Einstellungen
settings-aria-tabs = Einstellungsbereiche
settings-tab-profile = Profil
settings-tab-appearance = Darstellung
settings-tab-notifications = Benachrichtigungen
settings-tab-accessibility = Barrierefreiheit
settings-tab-help = Hilfe & Support
settings-coming-soon = Dieser Bereich ist bald verfügbar.

# Account menu & settings (account-menu feature)
account-menu-aria = Kontomenü
account-menu-profile = Profil & Status
account-menu-settings = Einstellungen
account-menu-shortcuts = Tastenkürzel
settings-a11y-dyslexic-label = Legasthenie-freundliche Schrift
settings-a11y-dyslexic-hint = Verwendet eine besser lesbare Schriftart für Dokumenttext.
settings-a11y-reduce-motion-label = Bewegung reduzieren
settings-a11y-reduce-motion-hint = Minimiert Animationen und Übergänge in der gesamten App.
settings-appearance-language = Sprache
# BYOK — bring-your-own Anthropic key (#29)
settings-byok-label = KI-Assistent – eigenen Anthropic-Schlüssel verwenden
settings-byok-hint = Wird nur in diesem Browser gespeichert und mit Ihren KI-Anfragen gesendet; niemals auf unseren Servern abgelegt. Leer lassen, um den Workspace-Schlüssel zu verwenden.
settings-byok-active = Ihr Schlüssel wird verwendet
settings-byok-none = Workspace-Schlüssel wird verwendet.
settings-byok-save = Schlüssel speichern
settings-byok-clear = Schlüssel entfernen
settings-appearance-theme = Design
settings-help-shortcuts = Tastenkürzel
settings-help-shortcut-palette = Befehlspalette / Suche öffnen
settings-help-shortcut-actions = Befehlspalette (Aktionen)
settings-help-version = Version
settings-notif-email-heading = E-Mail-Benachrichtigungen
settings-notif-all = Alle Aktivitäten
settings-notif-mentions = Nur Erwähnungen
settings-notif-off = Aus
settings-notif-hint = Legt fest, welche Aktivitäten E-Mails auslösen. In-App-Benachrichtigungen bleiben unberührt.
settings-profile-name = Anzeigename
settings-profile-avatar = Avatar-URL
settings-profile-email = E-Mail
settings-profile-email-hint = Ihre E-Mail-Adresse wird von Ihrem Anmeldeanbieter verwaltet und kann hier nicht geändert werden.
settings-save = Änderungen speichern
settings-saving = Wird gespeichert…
settings-saved = Gespeichert
settings-profile-error = Ihre Änderungen konnten nicht gespeichert werden. Bitte versuchen Sie es erneut.
settings-profile-load-error = Ihr Profil konnte nicht geladen werden. Laden Sie die Seite neu, um es erneut zu versuchen.
settings-profile-name-required = Der Anzeigename darf nicht leer sein.
settings-profile-avatar-invalid = Die Avatar-URL muss mit http:// oder https:// beginnen.
settings-status-heading = Status
settings-status-emoji = Status-Emoji
settings-status-text = Wie ist Ihr Status?
settings-status-expiry = Löschen nach
settings-status-expiry-never = Nicht löschen
settings-status-expiry-30m = 30 Minuten
settings-status-expiry-1h = 1 Stunde
settings-status-expiry-4h = 4 Stunden
settings-status-set = Status festlegen
settings-status-clear = Status löschen
theme-label-system = System
theme-label-light = Hell
theme-label-dark = Dunkel

# --- menu bar + editor context menu (i18n backfill) ---
menu-cut = Ausschneiden
menu-copy = Kopieren
menu-paste = Einfügen
menu-bold = Fett
menu-italic = Kursiv
menu-underline = Unterstrichen
menu-strikethrough = Durchgestrichen
menu-code = Code
menu-comment = Kommentar
menu-alignment = Ausrichtung
menu-align-left = Links
menu-align-center = Zentriert
menu-align-right = Rechts
menubar-doc-new = Neu
menubar-doc-share = Teilen…
menubar-doc-copy-link = Link kopieren
menubar-doc-move-folder = In Ordner verschieben…
menubar-doc-duplicate = Duplizieren…
menubar-doc-new-template = Neu aus Vorlage…
menubar-doc-export = Exportieren
menubar-doc-export-html = HTML
menubar-doc-export-markdown = Markdown (kopieren)
menubar-doc-export-csv = CSV
menubar-doc-export-excel = Excel (.xlsx)
menubar-doc-print = Drucken…
menubar-doc-history = Dokumentverlauf…
menubar-doc-details = Dokumentdetails…
menubar-doc-rename = Dokument umbenennen…
menubar-doc-delete = Dokument löschen…
menubar-edit-undo = Rückgängig
menubar-edit-redo = Wiederholen
menubar-edit-find = Suchen und Ersetzen
menubar-edit-copy-anchor = Ankerlink kopieren
menubar-view-comments = Kommentare anzeigen
menubar-view-conversation = Konversation anzeigen
menubar-view-cursors = Cursor anzeigen
menubar-view-focus = Fokusmodus
menubar-view-line-numbers = Zeilennummern anzeigen
menubar-view-page-breaks = Seitenumbrüche anzeigen
menubar-view-outline = Gliederung anzeigen
menubar-view-outline-expanded = Gliederung erweitert lassen
menubar-format-subscript = Tiefgestellt
menubar-format-superscript = Hochgestellt
menubar-format-text-color = Textfarbe
menubar-format-highlight = Hervorheben
menubar-format-paragraph-style = Absatzstil
menubar-format-list = Liste
menubar-format-clear = Formatierung entfernen
menubar-format-lock = Bearbeitung sperren
editorctx-paragraph-style = Absatzstil
editorctx-insert-link = Link einfügen…
