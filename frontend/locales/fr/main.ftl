# OgreNotes — French (fr) translations.
#
# Source-of-truth catalog is locales/en-US/main.ftl. Keys present
# in en-US but missing here fall back to en-US at runtime.

# ─── Common ─────────────────────────────────────────────────────

common-loading = Chargement…
common-send = Envoyer
common-close = Fermer
# Document Details panel (#141)
document-details-title = Détails du document
document-details-name = Nom
document-details-type = Type
document-details-created = Créé
document-details-modified = Dernière modification
document-details-words = Mots
document-details-characters = Caractères
doc-type-document = Document
doc-type-spreadsheet = Feuille de calcul
# Editor gutter (#139)
editor-page-break = Page { $n }
# Find & Replace bar (#147)
find-placeholder = Rechercher
find-replace-placeholder = Remplacer par
find-no-results = Aucun résultat
find-prev = Correspondance précédente
find-next = Correspondance suivante
find-replace = Remplacer
find-replace-all = Tout remplacer
common-delete = Supprimer
common-cancel = Annuler
common-untitled = Sans titre
common-redirecting-login = Redirection vers la connexion…
common-redirecting = Redirection…
common-open-navigation = Ouvrir la navigation
common-restore-here = Restaurer ici

# ─── Sidebar ────────────────────────────────────────────────────

sidebar-section-navigation = Navigation
# Favorites (#144)
document-favorite = Ajouter aux favoris
# Expand / full screen (#145)
document-expand-enter = Passer en plein écran
document-expand-exit = Quitter le plein écran
document-unfavorite = Retirer des favoris
sidebar-section-favorites = Favoris
sidebar-empty-favorites = Aucun favori
sidebar-doc-open-new-tab = Ouvrir dans un nouvel onglet
sidebar-doc-actions-aria = Actions du document
sidebar-new-aria = Créer
sidebar-new-document = Nouveau document
sidebar-new-spreadsheet = Nouvelle feuille de calcul
menubar-help = Aide
sidebar-home = Accueil
sidebar-search = Rechercher
sidebar-sign-out = Se déconnecter
sidebar-aria-main-nav = Navigation principale
sidebar-aria-collapse = Réduire la barre latérale
sidebar-aria-expand = Développer la barre latérale

# ─── Menu bar ───────────────────────────────────────────────────

menubar-document = Document
menubar-edit = Édition
menubar-view = Affichage
menubar-insert = Insérer
menubar-format = Format

# ─── Notification panel ─────────────────────────────────────────

notifications-title = Notifications
notifications-mark-all-read = Tout marquer comme lu
notifications-empty = Aucune notification

# ─── Chat panel ─────────────────────────────────────────────────

chat-section-title = Discussions
chat-empty = Pas encore de discussions
chat-back = ← Retour aux discussions
chat-message-placeholder = Saisissez un message…

# ─── Document outline ───────────────────────────────────────────

outline-title = Plan
outline-empty = Aucun titre
outline-aria-close = Fermer le plan

# ─── Editor toolbar ─────────────────────────────────────────────

toolbar-undo = Annuler (Ctrl+Z)
toolbar-redo = Rétablir (Ctrl+Shift+Z)
toolbar-bold = Gras (Ctrl+B)
toolbar-italic = Italique (Ctrl+I)
toolbar-underline = Souligné (Ctrl+U)
toolbar-strikethrough = Barré
# Subscript / Superscript (#143)
toolbar-subscript = Indice
toolbar-superscript = Exposant
toolbar-code = Code (Ctrl+E)
# Toolbar alignment controls (#134)
toolbar-align-left = Aligner à gauche
toolbar-align-center = Centrer
toolbar-align-right = Aligner à droite
toolbar-text-color = Couleur du texte
toolbar-remove-color = Supprimer la couleur
toolbar-highlight = Surligner
toolbar-remove-highlight = Supprimer le surlignage
toolbar-image = Image
toolbar-link = Lien (Ctrl+K)
toolbar-horizontal-rule = Ligne horizontale
toolbar-insert-table = Insérer un tableau
toolbar-insert-label = Insérer :
toolbar-comment = Commenter (Ctrl+Alt+C)
toolbar-comment-label = Commenter
toolbar-more = Plus
toolbar-aria-more = Plus d'options de la barre
toolbar-prompt-url = Saisissez l'URL :

# Block-type dropdown
toolbar-block-paragraph = Paragraphe
toolbar-block-heading-1 = Titre 1
toolbar-block-heading-2 = Titre 2
toolbar-block-heading-3 = Titre 3
toolbar-block-heading-4 = Titre 4
toolbar-block-bulleted-list = Liste à puces
toolbar-block-numbered-list = Liste numérotée
toolbar-block-checklist = Liste de tâches
toolbar-block-blockquote = Citation
toolbar-block-code-block = Bloc de code
toolbar-block-format = Format

# Number formats (spreadsheet block-type menu)
toolbar-num-general = Général
toolbar-num-integer = Entier
toolbar-num-decimal-1 = Décimal (1)
toolbar-num-decimal-2 = Décimal (2)
toolbar-num-thousands = Milliers
toolbar-num-currency-usd = Devise (USD)
toolbar-num-currency-eur = Devise (EUR)
toolbar-num-percent = Pourcentage

# ─── Comment popup ──────────────────────────────────────────────

comment-new-title = Nouveau commentaire
comment-thread-title = Fil de commentaires
comment-aria-prev = Commentaire précédent
comment-aria-next = Commentaire suivant
comment-placeholder-new = Ajoutez un commentaire sur cette section
comment-placeholder-reply = Saisissez un message…

# ─── Conversation pane ──────────────────────────────────────────

conversation-thread = Fil
conversation-comment-on-block = Commenter le bloc
conversation-comments = Commentaires
conversation-empty = Aucun commentaire pour l'instant. Lancez une conversation !
conversation-placeholder-block = Commenter ce bloc…
conversation-placeholder-add = Ajouter un commentaire…
conversation-placeholder-reply = Répondre…
conversation-aria-prev = Commentaire précédent
conversation-aria-next = Commentaire suivant
conversation-back = ← Retour
conversation-status-open = Ouvert
conversation-status-resolved = Résolu
conversation-resolve = Résoudre
conversation-reopen = Rouvrir
conversation-typing-1 = { $name } est en train d'écrire…
conversation-typing-2 = { $a } et { $b } sont en train d'écrire…
conversation-typing-many = Plusieurs personnes sont en train d'écrire…

# ─── History viewer ─────────────────────────────────────────────

history-title = Historique des modifications
history-empty = Pas encore d'historique des versions
history-no-prior = Aucune version antérieure à comparer — c'est la première capture.
history-changes-in-v = Modifications dans la v{ $version }
history-restore-version = Restaurer la version
history-aria-close = Fermer
history-jump-to-block-title = Aller au bloc dans le document en direct
history-jump-to-block-label = Aller au bloc ↗
history-restoring = Restauration…
history-restore-to-this-version = Restaurer cette version
history-restore-confirm-message = Remplacer le document actuel par cette version ? Toutes les modifications locales non enregistrées seront perdues.
history-restore-confirm-label = Restaurer
history-deleted-badge = (supprimé)

# Node-type labels for diff cards
node-paragraph = Paragraphe
node-heading = Titre
node-bullet-list = Liste à puces
node-ordered-list = Liste numérotée
node-list-item = Élément de liste
node-task-list = Liste de tâches
node-task-item = Tâche
node-blockquote = Citation
node-code-block = Bloc de code
node-horizontal-rule = Séparateur
node-image = Image
node-table = Tableau
node-table-row = Ligne de tableau
node-table-cell = Cellule de tableau
node-table-header = En-tête de tableau
node-block = Bloc

# ─── Login page ─────────────────────────────────────────────────

login-tagline = Des documents qui ont du mordant.
login-error-name-email-required = Le nom et l'e-mail sont obligatoires
login-placeholder-name = Nom affiché
login-placeholder-email = E-mail
login-signing-in = Connexion en cours…
login-dev-button = Connexion développeur (personnalisée)
login-github = Se connecter avec GitHub
login-google = Se connecter avec Google

# ─── Share dialog ───────────────────────────────────────────────

share-title = Partager
share-placeholder-email = Saisissez une adresse e-mail
share-button = Partager
share-members-heading = Membres actuels
share-role-owner = Propriétaire
share-role-edit = Peut modifier
share-role-comment = Peut commenter
share-role-view = Peut consulter
share-error-no-folder = Le document n'a pas de dossier — partage impossible
share-error-enter-email = Saisissez une adresse e-mail
share-status-searching = Recherche en cours…
share-error-search-failed = Échec de la recherche : { $err }
share-error-no-user = Aucun utilisateur trouvé avec l'e-mail « { $email } »
share-status-shared-with = Partagé avec { $name }
share-error-failed = Échec du partage : { $err }

# Partage par lien (section du dialogue de partage, au niveau du document)
share-link-heading = Partage par lien
share-link-mode-off = Désactivé
share-link-mode-view = Peut consulter
share-link-mode-edit = Peut modifier
share-link-note = Toute personne de votre espace de travail disposant du lien peut l'ouvrir.
share-link-off = Le partage par lien est désactivé.
share-link-opt-comments = Autoriser les commentaires
share-link-opt-history = Afficher l'historique des modifications
share-link-opt-conversation = Afficher la conversation
share-link-opt-request = Autoriser les demandes de modification
share-link-copy = Copier le lien
share-link-copied = Lien copié
share-link-saved = Enregistré
share-link-error = Échec de l'enregistrement : { $err }
# Viewer-facing request-edit-access affordance (#110)
share-link-request-banner = Vous consultez ce document via un lien partagé.
share-link-request-button = Demander l'accès en modification
share-link-request-sending = Envoi…
share-link-request-sent = Demande envoyée
share-link-request-retry = Échec de l'envoi — réessayer

# ─── Document page ──────────────────────────────────────────────

document-loading = Chargement du document…
document-trash-banner = Ce document est dans la corbeille — restaurez-le pour le modifier.
document-trash-restore = Restaurer
document-trash-delete-forever = Supprimer définitivement
document-share-tooltip = Partager
# Document menu: rename + move (#146)
document-rename-prompt = Renommer le document
document-move-folder-title = Déplacer vers un dossier
document-move-here = Déplacer ici
# Duplicate dialog (#146)
duplicate-dialog-title = Dupliquer le document
duplicate-name-label = Nom
duplicate-destination-label = Dossier de destination
duplicate-confirm = Dupliquer
duplicate-share-warning = Ce dossier est partagé — { $count } autres personnes y ont accès et verront la copie.
# Focus/expand toggle (#134)
document-focus-enter = Mode focus
document-focus-exit = Quitter le mode focus
document-trash-dialog-title = Déplacer vers la corbeille
document-trash-dialog-message = Ce document sera déplacé vers la corbeille. Vous pourrez le restaurer plus tard.
document-trash-dialog-confirm = Déplacer vers la corbeille
document-purge-dialog-title = Supprimer définitivement ?
document-purge-dialog-message = Cela supprime définitivement le document et tout son contenu. Cette action est irréversible.
document-purge-dialog-confirm = Supprimer définitivement
document-restore-folder-title = Restaurer dans un dossier

# ─── Home page ──────────────────────────────────────────────────

home-new-document = + Nouveau document
home-new-spreadsheet = + Nouvelle feuille de calcul
home-new-folder = + Nouveau dossier

# ─── MFA (enroll + challenge) ───────────────────────────────────

mfa-verifying = Vérification…
mfa-enter-totp = Saisissez le code à 6 chiffres de votre application d'authentification
mfa-enter-recovery = Saisissez votre code de récupération

mfa-enroll-title = Configurer l'authentification à deux facteurs
mfa-enroll-subtitle = Scannez le code QR avec votre application d'authentification, puis saisissez le code à 6 chiffres pour confirmer.
mfa-enroll-success = Inscription confirmée. Redirection…
mfa-enroll-error-failed = Échec de l'inscription : { $err }
mfa-enroll-error-verify-failed = Échec de la vérification : { $err }
mfa-enroll-manual-entry = Saisie manuelle
mfa-enroll-recovery-codes-summary = Codes de récupération (enregistrez-les dès maintenant !)
mfa-enroll-recovery-warning = Chaque code peut être utilisé une seule fois si vous perdez l'accès à votre application d'authentification. Nous ne les afficherons plus.
mfa-enroll-code-label = Code de l'application d'authentification
mfa-enroll-confirm = Confirmer

mfa-challenge-title = Authentification à deux facteurs
mfa-challenge-subtitle-totp = Ouvrez votre application d'authentification et saisissez le code à 6 chiffres.
mfa-challenge-subtitle-recovery = Saisissez l'un de vos codes de récupération à usage unique.
mfa-challenge-verify = Vérifier
mfa-challenge-missing-handle = Identifiant MFA manquant, redirection vers la connexion…
mfa-challenge-error-invalid-totp = Code invalide — vérifiez votre application d'authentification et réessayez
mfa-challenge-error-invalid-recovery = Code de récupération invalide — chaque code ne peut être utilisé qu'une seule fois
mfa-challenge-use-totp = Utiliser le code de l'application à la place
mfa-challenge-use-recovery = Vous avez perdu votre application ? Utilisez un code de récupération

# ─── Admin console (platform-admin pages) ───────────────────────

admin-loading = Chargement de la console d'administration…
admin-redirecting = Redirection…
admin-status-active = actif
admin-status-disabled = désactivé
admin-status-never = jamais
admin-role-admin = administrateur
admin-role-user = utilisateur
admin-retry = Réessayer

# Admin sub-nav
admin-nav-users = Utilisateurs
admin-nav-metrics = Métriques
admin-nav-audit = Audit
admin-nav-back = Retour à l'application

# Admin · Users
admin-users-title = Admin · Utilisateurs
admin-users-search-placeholder = Filtrer par préfixe d'e-mail
admin-users-th-email = E-mail
admin-users-th-name = Nom
admin-users-th-role = Rôle
admin-users-th-state = État
admin-users-th-last-active = Dernière activité
admin-users-th-actions = Actions
admin-users-enable = Activer
admin-users-disable = Désactiver
admin-users-promote = Promouvoir
admin-users-demote = Rétrograder
admin-users-prev = Préc.
admin-users-next = Suiv.
admin-users-error-list-failed = Échec de la liste : { $err }
admin-users-error-action-failed = Échec de { $action } : { $err }

# Admin · Audit
admin-audit-title = Admin · Journal d'audit
admin-audit-label-target = ID utilisateur cible
admin-audit-label-actor = ID utilisateur acteur
admin-audit-label-kind = Type
admin-audit-placeholder-kind = par ex. disable, loginFailure
admin-audit-label-from = Du (ISO)
admin-audit-label-to = Au (ISO)
admin-audit-search = Rechercher
admin-audit-error-target-required = L'ID utilisateur cible est obligatoire
admin-audit-error-load-failed = Échec du chargement : { $err }
admin-audit-th-when = Quand
admin-audit-th-source = Source
admin-audit-th-kind = Type
admin-audit-th-actor = Acteur
admin-audit-th-target = Cible
admin-audit-th-detail = Détail

# Admin · Metrics
admin-metrics-title = Admin · Métriques
admin-metrics-refresh = Actualiser
admin-metrics-error-fetch-failed = Échec de la récupération : { $err }
admin-metrics-counters = Compteurs
admin-metrics-gauges = Jauges
admin-metrics-histograms = Histogrammes
admin-metrics-th-key = Clé
admin-metrics-th-value = Valeur
admin-metrics-th-count = Nombre
admin-metrics-th-sum = Somme
admin-metrics-th-min = Min
admin-metrics-th-max = Max

# ─── Workspace SCIM tokens ──────────────────────────────────────

scim-title = Jetons SCIM de l'espace de travail
scim-subtitle = Créez un jeton bearer pour le connecteur d'approvisionnement SCIM de votre IdP. Le texte en clair n'est affiché qu'une seule fois à la création — copiez-le immédiatement.
scim-base-url-heading = URL de base SCIM
scim-base-url-help = Collez ceci dans la configuration du connecteur SCIM de votre IdP.
scim-fresh-heading = Nouveau jeton : { $name }
scim-fresh-warning = Copiez ce jeton MAINTENANT — il ne sera plus affiché.
scim-fresh-copy = Copier
scim-create-heading = Créer un nouveau jeton
scim-create-placeholder = Libellé (par ex. connecteur Okta)
scim-create-button = Créer
scim-existing-heading = Jetons existants
scim-empty = Pas encore de jetons. Créez-en un ci-dessus.
scim-th-name = Nom
scim-th-token-id = ID du jeton
scim-th-created = Créé le
scim-th-last-used = Dernière utilisation
scim-th-status = État
scim-status-active = actif
scim-status-revoked = révoqué
scim-revoke = Révoquer
scim-error-name-required = Le nom du jeton est obligatoire
scim-error-load-failed = Échec du chargement : { $err }
scim-error-create-failed = Échec de la création : { $err }
scim-error-revoke-failed = Échec de la révocation : { $err }

# ─── Workspace SAML SSO ─────────────────────────────────────────

saml-title = SSO SAML de l'espace de travail
saml-subtitle-prefix = Configurez un IdP SAML 2.0 pour cet espace de travail. Les membres pourront se connecter via l'IdP à l'adresse
saml-subtitle-suffix = une fois enregistré.
saml-status-saved = Configuration SAML enregistrée.
saml-status-removed = Configuration SAML supprimée.
saml-status-copied = URL des métadonnées SP copiée dans le presse-papiers.
saml-sp-heading = Métadonnées SP
saml-sp-help = Copiez cette URL dans le flux « ajouter un fournisseur de services » de votre IdP. Ou récupérez l'URL une fois et collez le XML de réponse dans votre IdP.
saml-copy = Copier
saml-idp-heading = Configuration IdP
saml-idp-help = Collez le XML des métadonnées fourni par votre IdP. Le XML complet est requis — y compris l'élément racine <EntityDescriptor>.
saml-label-entity-id = Entity ID de l'IdP
saml-placeholder-entity-id = https://idp.example.com/metadata
saml-label-metadata-xml = XML des métadonnées de l'IdP
saml-label-email-attr = Nom de l'attribut e-mail
saml-label-name-attr = Nom de l'attribut nom
saml-save = Enregistrer
saml-update = Mettre à jour
saml-remove = Supprimer
saml-error-entity-id-required = L'Entity ID de l'IdP est obligatoire
saml-error-metadata-required = Le XML des métadonnées de l'IdP est obligatoire
saml-error-load-failed = Échec du chargement : { $err }
saml-error-save-failed = Échec de l'enregistrement : { $err }
saml-error-delete-failed = Échec de la suppression : { $err }
saml-meta-first-configured = Première configuration le
saml-meta-last-updated = ; dernière mise à jour le

# ─── Spreadsheet view (chrome) ──────────────────────────────────

ss-empty = Aucune donnée
ss-format-painter-title = Reproduire la mise en forme — cliquez pour copier la mise en forme de la cellule active, puis cliquez sur une cible. Maj+clic pour le mode permanent.
ss-format-painter-status = Reproduction — cliquez sur une cellule pour appliquer, Échap pour annuler
ss-format-painter-status-sticky = Reproduction (permanente) — cliquez sur des cellules pour appliquer, Échap pour arrêter
ss-sort-tooltip = Trier la feuille de calcul…

# Status bar
ss-status-count = Nombre : { $value }
ss-status-sum = Somme : { $value }
ss-status-avg = Moy. : { $value }
ss-status-min = Min : { $value }
ss-status-max = Max : { $value }

# Sheet tabs
ss-rename-sheet-prompt = Renommer la feuille :
ss-touch-menu-aria = Actions de cellule
ss-ctx-rename = Renommer
ss-ctx-delete = Supprimer

# Find / replace bar
ss-find-placeholder = Rechercher…
ss-replace-placeholder = Remplacer…
ss-find-next = Suivant
ss-find-replace = Remplacer
ss-find-replace-all = Tout remplacer
ss-find-no-results = 0 résultat

# Filter dropdown
ss-filter-header = Filtrer : { $col }
ss-filter-show-all = Tout afficher
ss-filter-custom-prompt = Filtre personnalisé (par ex. >100, <0, =Done, contains:err, empty, notempty) :
ss-filter-custom-button = Filtre personnalisé…
ss-filter-empty-value = (vide)

# Sort dialog
ss-sort-title = Trier
ss-sort-range-label = Plage :
ss-sort-has-headers = La première ligne contient des en-têtes (à ignorer lors du tri)
ss-sort-by-label = Trier par
ss-sort-then-by-label = Puis par
ss-sort-asc = Croissant
ss-sort-desc = Décroissant
ss-sort-remove-level-title = Supprimer ce niveau de tri
ss-sort-add-level = + Ajouter un niveau de tri
ss-sort-cancel = Annuler
ss-sort-apply = Appliquer
ss-sort-err-parse-range = Impossible d'analyser la plage. Utilisez la notation A1, par ex. A1:G41.
ss-sort-err-no-keys = Ajoutez au moins une clé de tri.

# Foreign-document consent dialog
ss-foreign-title = Ce document récupère des données depuis d'autres classeurs
ss-foreign-hint = Autoriser les récupérations utilise l'accès en lecture de votre compte à ces documents. L'approbation ne dure que pour cette session.
ss-foreign-deny = Refuser
ss-foreign-allow = Autoriser

# ─── Spreadsheet context menu (cell right-click) ────────────────

ss-ctx-menu-insert = Insérer
ss-ctx-menu-delete = Supprimer
ss-ctx-menu-sort = Trier
ss-ctx-menu-format = Format
ss-ctx-menu-comment = Commentaire
ss-ctx-menu-hide = Masquer / Afficher
ss-ctx-menu-data = Données
ss-ctx-menu-cond-fmt = Mise en forme conditionnelle
ss-ctx-menu-validation = Validation des données
ss-ctx-insert-row-above = Insérer une ligne au-dessus
ss-ctx-insert-row-below = Insérer une ligne en dessous
ss-ctx-insert-col-left = Insérer une colonne à gauche
ss-ctx-insert-col-right = Insérer une colonne à droite
ss-ctx-sort-dialog = Trier…
ss-ctx-delete-row = Supprimer la ligne
ss-ctx-delete-rows = Supprimer { $count } lignes
ss-ctx-delete-col = Supprimer la colonne
ss-ctx-delete-cols = Supprimer { $count } colonnes
ss-ctx-clear-contents = Effacer le contenu
ss-ctx-sort-a-z = Trier A → Z
ss-ctx-sort-z-a = Trier Z → A
ss-ctx-freeze-rows = Figer les lignes au-dessus
ss-ctx-unfreeze-rows = Libérer les lignes
ss-ctx-freeze-cols = Figer les colonnes à gauche
ss-ctx-unfreeze-cols = Libérer les colonnes

# Cell validation
ss-ctx-set-checkbox = Définir comme case à cocher
ss-ctx-set-dropdown = Définir comme menu déroulant…
ss-ctx-remove-validation = Supprimer la validation
ss-ctx-dropdown-prompt = Saisissez les options du menu (séparées par des virgules) :

# Conditional formatting
ss-ctx-cond-fmt = Mise en forme conditionnelle…
ss-ctx-cond-fmt-prompt = Mise en forme conditionnelle (par ex. >100, <0, =Done, contains:error, empty, notempty) :
ss-ctx-cond-fmt-color-prompt = Couleur d'arrière-plan (par ex. #ff0000, red, #ffd) :
ss-ctx-color-scale = Échelle de couleurs…
ss-ctx-color-scale-prompt = Échelle de couleurs : bas,haut ou bas,milieu,haut (par ex. #ff0000,#ffff00,#00ff00) :
ss-ctx-data-bar = Barre de données…
ss-ctx-data-bar-prompt = Couleur de la barre de données :
ss-ctx-icon-set = Jeu d'icônes…
ss-ctx-icon-set-prompt = Jeu d'icônes : arrows ou traffic

# Charts + pivots
ss-ctx-insert-chart = Insérer un graphique…
ss-ctx-chart-type-prompt = Type de graphique (bar, line, pie) :
ss-ctx-chart-title-prompt = Titre du graphique :
ss-ctx-chart-unknown-type = Type de graphique inconnu. Utilisez l'un de : bar, line, pie.
ss-ctx-insert-pivot = Insérer un tableau croisé dynamique…
ss-ctx-pivot-needs-multi = Le tableau croisé dynamique nécessite une sélection sur plusieurs lignes et plusieurs colonnes. Sélectionnez vos données (avec la ligne d'en-tête en ligne 1) et réessayez.

# CSV import + merge
ss-ctx-import-csv = Importer un CSV…
ss-ctx-merge-cells = Fusionner les cellules
ss-ctx-unmerge-cells = Annuler la fusion

# Hide / unhide
ss-ctx-hide-row = Masquer la ligne
ss-ctx-unhide-all-rows = Afficher toutes les lignes
ss-ctx-hide-col = Masquer la colonne
ss-ctx-unhide-all-cols = Afficher toutes les colonnes

# Cell lock + comments + named ranges
ss-ctx-lock-cell = Verrouiller la cellule
ss-ctx-unlock-cell = Déverrouiller la cellule
ss-ctx-add-comment = Ajouter un commentaire…
ss-ctx-edit-comment = Modifier le commentaire…
ss-ctx-open-comment = Ouvrir le fil de commentaires…
ss-ctx-comment-prompt = Commentaire :
ss-ctx-remove-comment = Supprimer le commentaire
ss-comment-preview-empty = Aucun message pour l'instant
ss-ctx-define-name = Définir un nom…
ss-ctx-name-prompt = Nom pour cette plage :
ss-ctx-remove-name = Supprimer un nom…
ss-ctx-no-named-ranges = Aucune plage nommée définie.
ss-ctx-remove-name-prompt = Quel nom supprimer ? Définis : { $names }

# ─── Pivot table editor ─────────────────────────────────────────

ss-pivot-title = Éditeur de tableau croisé dynamique
ss-pivot-foreign-source-label = Source externe :
ss-pivot-foreign-hint = La modification de la source externe n'est pas encore prise en charge. Modifiez la configuration du tableau croisé via l'attribut JSON ou supprimez-le et recréez-le en tant que tableau croisé local.
ss-pivot-layout = Disposition
ss-pivot-layout-compact = Compacte
ss-pivot-layout-outline = Plan
ss-pivot-layout-tabular = Tabulaire
ss-pivot-totals = Totaux
ss-pivot-totals-none = Aucun
ss-pivot-totals-rows = Lignes
ss-pivot-totals-cols = Cols
ss-pivot-totals-both = Les deux
ss-pivot-edit-filter-tooltip = Modifier le filtre
ss-pivot-axis-row = Ligne
ss-pivot-axis-col = Colonne
ss-pivot-labels-header = Étiquettes { $axis } — { $col }
ss-pivot-close-tooltip = Fermer
ss-pivot-close-editor-tooltip = Fermer l'éditeur (le tableau croisé reste affiché)
ss-pivot-filter-all = Tous
ss-pivot-filter-none = Aucun
ss-pivot-filter-prefix = Filtrer { $col }
ss-pivot-source-label = Source :
ss-pivot-delete = Supprimer le tableau croisé
ss-pivot-section-fields = Champs
ss-pivot-section-rows = Lignes
ss-pivot-section-cols = Colonnes
ss-pivot-section-values = Valeurs
ss-pivot-section-filters = Filtres
ss-pivot-search-placeholder = Rechercher des champs…
ss-pivot-bin-width-tooltip = Largeur de bac

# Date granularity options
ss-pivot-date-year = Année
ss-pivot-date-quarter = Trimestre
ss-pivot-date-month = Mois
ss-pivot-date-day = Jour
ss-pivot-date-hour = Heure

# ─── App-level / router ─────────────────────────────────────────

app-not-found = Page introuvable

# ─── Accessibility ──────────────────────────────────────────────

a11y-skip-to-content = Aller au contenu principal
a11y-toolbar-label = Barre de mise en forme du document
a11y-toolbar-group-undo = Annuler et rétablir
a11y-toolbar-group-block-type = Type de bloc
a11y-toolbar-group-inline = Mise en forme en ligne
a11y-toolbar-group-align = Alignement
a11y-toolbar-group-block = Mise en forme de bloc
a11y-toolbar-group-insert = Insérer
a11y-file-table-label = Documents et dossiers
a11y-breadcrumb-label = Chemin des dossiers

# ─── @-menu (mention picker) ────────────────────────────────────

at-menu-empty = Saisissez pour rechercher des personnes et des documents…

# ─── File browser (home page table) ─────────────────────────────

file-browser-empty = Rien ici pour l'instant. Créez un document ou un dossier.
file-browser-th-title = Titre
file-browser-th-added = Ajouté
file-type-folder = Dossier
file-type-document = Document
file-type-spreadsheet = Feuille de calcul
file-type-chat = Discussion

# ─── Folder picker ──────────────────────────────────────────────

folder-picker-not-available =  (non disponible)

# ─── Formula keyboard ───────────────────────────────────────────

formula-key-backspace = Retour arrière
formula-key-cancel = Annuler (Échap)
formula-key-commit = Valider (Entrée)
# Mode-switcher tabs (Phase 5 M-P3 piece C).
kbd-mode-standard = Aa
kbd-mode-numeric = 123
kbd-mode-formula = ƒx
kbd-standard-hint = Utilisez le clavier de votre appareil

# ─── Search dialog ──────────────────────────────────────────────

search-placeholder = Rechercher des documents…
search-searching = Recherche en cours…
search-no-results = Aucun résultat
search-dialog-label = Rechercher des documents ou exécuter des commandes

# ─── Ask (assistant RAG) ────────────────────────────────────────

ask-dialog-title = Demander à l'assistant
ask-badge = IA
ask-placeholder = Posez une question sur vos documents…
ask-empty-hint = L'assistant cherche dans vos documents et cite ce qu'il trouve. Posez une question concrète.
ask-sources-heading = Sources
ask-error-rate-limit = Trop de requêtes. Patientez un instant et réessayez.
ask-error-disabled = Un administrateur a désactivé l'assistant pour votre espace de travail.
ask-error-unavailable = L'assistant est temporairement indisponible.
sidebar-ask = Demander

# ─── Relationships ──────────────────────────────────────────────

relationship-heading = Liés
relationship-empty = Aucun document lié pour l'instant.
relationship-add-aria = Ajouter un document lié
relationship-remove-aria = Supprimer cette relation
relationship-picker-placeholder = Rechercher des documents à lier…
relationship-picker-aria = Rechercher des documents à lier
relationship-picker-confirm = Lier
relationship-type-aria = Type de relation
relationship-error-self = Un document ne peut pas se lier à lui-même.
relation-type-implements = Implémente
relation-type-derived-from = Dérivé de
relation-type-depends-on = Dépend de
relation-type-references = Référence
relation-type-supersedes = Remplace

# ─── Theme selector ─────────────────────────────────────────────

theme-aria-label = Thème
theme-system = Suivre le thème du système
theme-light = Thème clair
theme-dark = Thème sombre

# ─── Locale selector ────────────────────────────────────────────

locale-aria-label = Langue

# ─── Inline selection / comment-bubble ──────────────────────────

selection-toolbar-comment = Commenter la sélection
comment-highlights-add = Ajouter un commentaire

# ─── Auth-callback page ─────────────────────────────────────────

auth-complete-status = Finalisation de la connexion…

# ─── Sync indicator (Phase 5 M-P3 piece B) ──────────────────────

sync-saved = Enregistré
sync-saving = Enregistrement…
sync-offline = Hors ligne
sync-offline-pending = Hors ligne — {$count} en attente
sync-saved-tooltip = Vos modifications sont enregistrées.
sync-saving-tooltip = Envoi de vos dernières modifications au serveur…
sync-offline-tooltip = Vous êtes déconnecté. Reconnectez-vous pour continuer à collaborer.
sync-offline-pending-tooltip = Vous êtes déconnecté. {$count} modification(s) ne sont pas encore arrivées au serveur.
sync-not-saved = Non enregistré
sync-not-saved-tooltip = Le serveur a reçu vos dernières modifications mais n’a pas pu les enregistrer : il a perdu la connexion au stockage. Gardez cet onglet ouvert — elles seront renvoyées automatiquement dès le retour de la connexion.
storage-offline-title = Enregistrement impossible pour le moment.
storage-offline-body = Le serveur a perdu la connexion au stockage. Les documents ne peuvent être ni ouverts ni enregistrés avant son retour ; ce message disparaîtra de lui-même.
storage-boot-body = Le serveur a perdu la connexion au stockage et ne peut pas encore vous connecter. Cette page réessaie et reprendra d’elle-même au retour de la connexion.

# ─── Command palette (Phase 5 M-P4 piece A) ─────────────────────

palette-no-actions = Aucune commande ne correspond.
cmd-go-home = Aller à l'accueil
cmd-toggle-dark-mode = Basculer le mode sombre
cmd-open-trash = Ouvrir la corbeille
cmd-sign-out = Se déconnecter
cmd-ask = Demander à l'assistant
cmd-about-palette = Palette de commandes : à propos
# Editor-scoped commands (M-P4 piece B).
cmd-bold = Gras
cmd-italic = Italique
cmd-underline = Souligné
cmd-strike = Barré
cmd-code = Code en ligne
cmd-heading-1 = Titre 1
cmd-heading-2 = Titre 2
cmd-heading-3 = Titre 3
cmd-paragraph = Paragraphe
cmd-bullet-list = Liste à puces
cmd-ordered-list = Liste numérotée
cmd-task-list = Liste de tâches
cmd-blockquote = Citation
cmd-code-block = Bloc de code
cmd-divider = Insérer un séparateur
cmd-insert-table = Insérer un tableau
cmd-undo = Annuler
cmd-redo = Rétablir

# ─── Home-page drop-to-import (Phase 5 M-P5 piece D) ────────────

home-drop-title = Déposez pour importer
home-drop-hint = Markdown (.md) ou HTML (.html) — jusqu'à 1 Mo
home-import-default-title = Importé

# ─── Toolbar — embed insert (Phase 5 M-P6 piece B) ──────────────

toolbar-embed = Intégrer un média (URL)

# ─── File-browser bulk selection (Phase 5 M-P7 piece C) ─────────

file-browser-th-select = Sélectionner
bulk-selection-count = {$count} sélectionnés
bulk-selection-cancel = Annuler
bulk-selection-delete = Supprimer
bulk-delete-dialog-title = Déplacer les documents sélectionnés vers la corbeille ?
bulk-delete-dialog-message = Les documents sélectionnés seront déplacés vers votre corbeille. Vous pourrez les restaurer dans les 30 jours.
bulk-delete-dialog-confirm = Déplacer vers la corbeille

# ─── Account settings page (design/account-menu.md, step 1) ─────

settings-title = Paramètres
settings-aria-tabs = Sections des paramètres
settings-tab-profile = Profil
settings-tab-appearance = Apparence
settings-tab-notifications = Notifications
settings-tab-accessibility = Accessibilité
settings-tab-help = Aide et assistance
settings-coming-soon = Cette section sera bientôt disponible.

# Account menu & settings (account-menu feature)
account-menu-aria = Menu du compte
account-menu-profile = Profil et statut
account-menu-settings = Paramètres
account-menu-shortcuts = Raccourcis clavier
settings-a11y-dyslexic-label = Police adaptée à la dyslexie
settings-a11y-dyslexic-hint = Utilise une police plus lisible pour le texte des documents.
settings-a11y-reduce-motion-label = Réduire les animations
settings-a11y-reduce-motion-hint = Réduit les animations et les transitions dans toute l'application.
settings-appearance-language = Langue
# BYOK — bring-your-own Anthropic key (#29)
settings-byok-label = Assistant IA — utiliser votre propre clé Anthropic
settings-byok-hint = Conservée uniquement dans ce navigateur et envoyée avec vos requêtes IA ; jamais stockée sur nos serveurs. Laissez vide pour utiliser la clé de l'espace de travail.
settings-byok-active = Votre clé est utilisée
settings-byok-none = Clé de l'espace de travail utilisée.
settings-byok-save = Enregistrer la clé
settings-byok-clear = Supprimer la clé
settings-appearance-theme = Thème
settings-help-shortcuts = Raccourcis clavier
settings-help-shortcut-palette = Ouvrir la palette de commandes / recherche
settings-help-shortcut-actions = Palette de commandes (actions)
settings-help-version = Version
settings-notif-email-heading = Notifications par e-mail
settings-notif-all = Toute l'activité
settings-notif-mentions = Mentions uniquement
settings-notif-off = Désactivées
settings-notif-hint = Détermine quelle activité vous envoie des e-mails. Les notifications dans l'application ne sont pas affectées.
settings-profile-name = Nom affiché
settings-profile-avatar = URL de l'avatar
settings-profile-email = E-mail
settings-profile-email-hint = Votre e-mail est géré par votre fournisseur de connexion et ne peut pas être modifié ici.
settings-save = Enregistrer les modifications
settings-saving = Enregistrement…
settings-saved = Enregistré
settings-profile-error = Impossible d'enregistrer vos modifications. Veuillez réessayer.
settings-profile-load-error = Impossible de charger votre profil. Rechargez la page pour réessayer.
settings-profile-name-required = Le nom affiché ne peut pas être vide.
settings-profile-avatar-invalid = L'URL de l'avatar doit commencer par http:// ou https://.
settings-status-heading = Statut
settings-status-emoji = Emoji de statut
settings-status-text = Quel est votre statut ?
settings-status-expiry = Effacer après
settings-status-expiry-never = Ne pas effacer
settings-status-expiry-30m = 30 minutes
settings-status-expiry-1h = 1 heure
settings-status-expiry-4h = 4 heures
settings-status-set = Définir le statut
settings-status-clear = Effacer le statut
theme-label-system = Système
theme-label-light = Clair
theme-label-dark = Sombre

# --- menu bar + editor context menu (i18n backfill) ---
menu-cut = Couper
menu-copy = Copier
menu-paste = Coller
menu-copy-block-link = Copier le lien du bloc
menu-copy-original-url = Copier l'URL d'origine
menu-convert-to-plain-link = Convertir en lien simple
menu-bold = Gras
menu-italic = Italique
menu-underline = Souligné
menu-strikethrough = Barré
menu-code = Code
menu-comment = Commenter
menu-alignment = Alignement
menu-align-left = Gauche
menu-align-center = Centre
menu-align-right = Droite
menubar-doc-new = Nouveau
menubar-doc-share = Partager…
menubar-doc-copy-link = Copier le lien
menubar-doc-move-folder = Déplacer vers un dossier…
menubar-doc-duplicate = Dupliquer…
menubar-doc-new-template = Nouveau depuis un modèle…
menubar-doc-export = Exporter
menubar-doc-export-html = HTML
menubar-doc-export-markdown = Markdown (copier)
menubar-doc-export-csv = CSV
menubar-doc-export-excel = Excel (.xlsx)
menubar-doc-print = Imprimer…
menubar-doc-history = Historique du document…
menubar-doc-details = Détails du document…
menubar-doc-rename = Renommer le document…
menubar-doc-delete = Supprimer le document…
menubar-edit-undo = Annuler
menubar-edit-redo = Rétablir
menubar-edit-find = Rechercher et remplacer
menubar-view-comments = Afficher les commentaires
menubar-view-conversation = Afficher la conversation
menubar-view-cursors = Afficher les curseurs
menubar-view-focus = Mode focus
menubar-view-line-numbers = Afficher les numéros de ligne
menubar-view-page-breaks = Afficher les sauts de page
menubar-view-outline = Afficher le plan
menubar-format-subscript = Indice
menubar-format-superscript = Exposant
menubar-format-paragraph-style = Style de paragraphe
menubar-format-list = Liste
menubar-format-clear = Effacer la mise en forme
menubar-format-lock = Verrouiller les modifications
editorctx-paragraph-style = Style de paragraphe
editorctx-insert-link = Insérer un lien…

# Editor width toggle (S/M/L)
editor-width-group = Largeur de l'éditeur
editor-width-narrow = Largeur étroite
editor-width-medium = Largeur moyenne
editor-width-wide = Grande largeur

# ─── Block deep links (#b= fragment consumption) ─────────────────

doc-block-link-missing = La section liée n'existe plus.

# ─── Mentions spec §5 (Task 5) — per-viewer degradation overlay ──

doc-mention-missing = Document introuvable

# ─── Quip import wizard (Phase 0) — shell-mounted, opens from the
# sidebar "+ New" menu. Step 1: paste a token, connect, see the
# profile + a root-folder checklist. The scope Continue is disabled
# until Phase 1 wires the actual import.
quip-import-menu-entry = Importer depuis Quip…
quip-import-title = Importer depuis Quip
quip-import-token-label = Jeton API Quip
quip-import-token-placeholder = Collez votre jeton d'accès personnel Quip
quip-import-token-hint = Besoin d’un jeton ?
quip-import-token-hint-link = Obtenez-en un sur Quip
quip-import-connect = Connecter
quip-import-connecting = Connexion…
quip-import-error = Échec de la connexion : { $err }
quip-import-profile = Connecté en tant que { $name }
quip-import-folder-scope-heading = Choisissez les dossiers à importer
quip-import-no-folders = Aucun dossier trouvé dans ce compte Quip.
quip-import-continue = Continuer
quip-import-target-home = Les documents seront importés dans un nouveau dossier Quip Import de votre dossier Accueil.
quip-import-target-folder = Les documents seront importés dans un nouveau dossier Quip Import situé dans { $folder }.
quip-import-destination-change = Modifier
quip-import-destination-heading = Choisir la destination de l'import
quip-import-destination-hint = Choisissez le dossier de destination. Un nouveau dossier Quip Import y sera créé, ce qui vous permet d'annuler tout l'import en supprimant ce seul dossier.
quip-import-destination-select = Utiliser ce dossier
quip-import-starting = Démarrage de l'import…
quip-import-scanning = Analyse de Quip… { $total } éléments trouvés
quip-import-inventory-done = { $total } éléments trouvés à importer
quip-import-estimate = Environ { $minutes } min compte tenu de la limite de débit de Quip
quip-import-import-failed = Échec de l'import. Veuillez réessayer.
quip-import-token-expired = Votre session Quip a expiré — veuillez vous reconnecter.
quip-import-importing = Import en cours… { $done } sur { $total }
quip-import-content-done = { $total } éléments importés
quip-import-open-folder = Ouvrir le dossier
quip-import-report-imported = { $imported } documents importés
quip-import-report-skipped = { $count } ignorés — vous n'y avez pas accès dans Quip
quip-import-report-failed = { $count } n'ont pas pu être importés
quip-import-report-and-more = …et { $count } de plus
quip-import-report-chat = { $count } fils de discussion n'ont pas été importés — les chats ne sont pas des documents
quip-import-report-note = { $id } — { $detail }
quip-import-report-note-general = { $detail }
quip-import-report-within-heading = Ces documents ont été importés, mais une partie de leur contenu n'a pas pu être reprise
quip-import-report-images = { $count } images n'ont pas pu être copiées depuis Quip
quip-import-report-truncated = Dans { $count } documents, le contenu profondément imbriqué a été aplati
quip-import-report-mentions = Dans { $count } documents, les @mentions ont été importées en texte brut
quip-import-report-live-apps = { $count } applications Quip intégrées (tableaux Kanban et similaires) n'ont pas pu être reprises
quip-import-report-formulas = { $count } formules de tableur n'ont pas été importées — ces cellules conservent leur dernière valeur et ne seront pas recalculées

# Additional editor, template, presentation, and language controls

document-favorite-menu = Favoris
favorite-menu-added = Ajouté aux favoris
favorite-menu-add = Ajouter aux favoris
favorite-menu-remove = Retirer des favoris
favorite-menu-collections = Collections
favorite-menu-new-collection = Nouvelle collection…
collection-new-prompt = Nommer cette collection
sidebar-empty-collection = Collection vide
sidebar-templates = Modèles
menubar-mark-template = Marquer comme modèle…
menubar-unmark-template = Retirer le statut de modèle
document-use-template = Utiliser le modèle
template-picker-title = Choisir un modèle
template-picker-loading = Chargement des modèles…
template-picker-empty = Aucun modèle. Marquez un document via Document → Marquer comme modèle…
template-picker-use = Utiliser
template-picker-using = Copie en cours…
template-picker-fill = Remplir…
template-picker-fill-title = Remplir les valeurs du modèle
template-picker-fill-hint = Laissez un champ vide pour conserver le texte de remplacement.
template-picker-create = Créer
template-picker-cancel = Annuler
template-picker-back = Retour à la liste des modèles
template-picker-section-mine = Vos modèles
template-picker-section-shared = Partagés avec vous
template-picker-section-sample = Exemples
template-picker-section-untagged = Modèles
modal-close = Fermer
notifications-clear-all = Tout effacer
notifications-dismiss = Ignorer
menubar-insert-link = Lien…
menubar-insert-table = Tableau
comment-edited = modifié
comment-edit = Édition
comment-save = Enregistrer
document-locked-banner = Ce document est verrouillé. La modification est désactivée jusqu’à son déverrouillage.
document-locked-unlock = Déverrouiller
document-folder-add = + Ajouter à un dossier
document-folder-add-title = Ajouter à un dossier
document-folder-add-confirm = Ajouter ici
document-folder-remove = Retirer de ce dossier
home-new-from-template = + Nouveau à partir d’un modèle
home-new-presentation = + Nouvelle présentation
folder-action-rename = Renommer
folder-action-delete = Supprimer
folder-rename-prompt = Renommer le dossier
folder-delete-dialog-title = Supprimer le dossier ?
folder-delete-dialog-message = Ce dossier vide sera supprimé définitivement. Cette action est irréversible.
folder-delete-dialog-confirm = Supprimer le dossier
folder-delete-not-empty = Ce dossier n’est pas vide. Déplacez ou retirez son contenu avant de le supprimer.
ss-ctx-copy-markdown = Copier au format Markdown
ss-multi-region-copy = La copie et la coupe nécessitent une seule sélection rectangulaire. Effacez les zones supplémentaires avec Échap et réessayez.
ss-multi-region-op = Cette action ne fonctionne pas avec une sélection multiple. Effacez les zones supplémentaires avec Échap et réessayez.
at-menu-section-people = Personnes
at-menu-section-documents = Documents
at-menu-section-insert = Insérer
at-menu-section-ai = IA
at-menu-ask-ai-hint = Demander à l’assistant…
at-menu-ai-summarize = Résumer ceci
at-menu-ai-translate = Traduire ceci
at-menu-ai-rewrite = Réécrire ceci
at-menu-ai-brainstorm = Trouver des idées
at-menu-insert-date-medium = Date d’aujourd’hui ({ $date })
at-menu-insert-date-short = Date d’aujourd’hui ({ $date })
at-menu-insert-date-long = Date et heure d’aujourd’hui
at-menu-insert-date-iso = Date d’aujourd’hui (ISO 8601)
insert-table-label = Tableau
insert-table-description = Insérer un tableau de 3×3
insert-image-label = Image
insert-image-description = Importer et insérer une image
insert-horizontal-rule-label = Séparateur
insert-horizontal-rule-description = Insérer une ligne horizontale
insert-code-block-label = Bloc de code
insert-code-block-description = Transformer ce bloc en bloc de code
ask-insert-into-document = Insérer dans le document
insert-calendar-label = Calendrier
insert-calendar-description = Calendrier intégré avec vues mensuelle, hebdomadaire et quotidienne
calendar-view-month = Mois
calendar-view-week = Semaine
calendar-view-day = Jour
calendar-nav-prev = Précédent
calendar-nav-next = Suiv.
calendar-nav-today = Aujourd’hui
calendar-empty-day = Cliquez pour ajouter un événement
calendar-event-untitled = (sans titre)
calendar-modal-add-title = Nouvel événement
calendar-modal-edit-title = Modifier l’événement
calendar-modal-content-label = Titre
calendar-modal-color-label = Couleur
calendar-modal-allday-label = Toute la journée
calendar-all-day-strip = toute la journée
calendar-modal-start-label = Début
calendar-modal-end-label = Fin
calendar-modal-save = Enregistrer
calendar-modal-delete = Supprimer
insert-kanban-label = Tableau Kanban
insert-kanban-description = Colonnes de cartes pour suivre le travail en cours
kanban-add-column = Ajouter une colonne
kanban-add-card = Ajouter une carte
kanban-delete-column = Supprimer la colonne
kanban-untitled-column = Colonne sans titre
kanban-untitled-card = (sans titre)
kanban-modal-add-title = Nouvelle carte
kanban-modal-edit-title = Modifier la carte
kanban-modal-title-label = Titre
kanban-modal-content-label = Description
kanban-modal-due-label = Date d’échéance
kanban-modal-labels-label = Étiquettes (nom|couleur ; …)
kanban-modal-assignee-label = Responsable
kanban-column-rename-prompt = Renommer la colonne
insert-mermaid-label = Diagramme Mermaid
insert-mermaid-description = Insérer un diagramme à partir de texte Mermaid
insert-math-label = Équation
insert-math-description = Insérer une équation écrite en LaTeX
kanban-column-delete-confirm = Supprimer cette colonne et toutes ses cartes ?
kanban-column-wip-limit-prompt = Limite de travail en cours (vide pour supprimer)
mermaid-modal-source-label = Source du diagramme
mermaid-modal-title = Modifier le diagramme
mermaid-modal-save = Enregistrer
mermaid-modal-error-empty = Le texte du diagramme ne peut pas être vide.
mermaid-modal-error-too-long = Le texte du diagramme est trop long (limite de { $max } caractères).
mermaid-modal-error-changed = Ce diagramme a changé pendant la modification. Copiez votre brouillon, puis rouvrez le diagramme pour modifier la dernière version.
mermaid-modal-error-unavailable = Ce diagramme ne peut plus être modifié. Copiez votre brouillon avant de fermer.
math-modal-title = Modifier l’équation
math-modal-save = Enregistrer
math-modal-source-label = Source LaTeX
math-modal-error-empty = L’équation ne peut pas être vide.
math-modal-error-too-long = L’équation est trop longue (limite de { $max } caractères).
settings-appearance-doc-theme = Typographie du document
settings-doc-theme-aria = Thème typographique du document
settings-doc-theme-default = Par défaut (Inter)
settings-doc-theme-editorial = Éditorial (Playfair Display / Source Serif)
settings-doc-theme-handwritten = Manuscrit (Caveat / Nunito)
settings-doc-theme-technical = Technique (JetBrains Mono)
settings-doc-theme-classic = Classique (Merriweather)
collab-liveapp-rejected-toast = Votre dernière modification n’a pas pu être enregistrée. Actualisez pour voir l’état actuel.
menubar-doc-export-pdf = PDF
deck-layout-title-label = Titre
deck-layout-title-content-label = Titre et contenu
deck-layout-two-column-label = Deux colonnes
deck-layout-blank-label = Vide
deck-placeholder-title = Cliquez pour ajouter un titre
deck-placeholder-subtitle = Cliquez pour ajouter un sous-titre
deck-placeholder-heading = Cliquez pour ajouter un en-tête
deck-placeholder-body = Cliquez pour ajouter du texte
deck-embed-chip-fallback = Contenu intégré
deck-placeholder-column = Cliquez pour ajouter du texte
deck-theme-slate-label = Ardoise
deck-theme-paper-label = Papier
deck-theme-midnight-label = Minuit
deck-theme-ember-label = Braise
deck-theme-forest-label = Forêt
deck-theme-ocean-label = Océan
deck-new = Nouvelle présentation
deck-doc-type-label = Présentation
deck-add-slide = Ajouter une diapositive
deck-duplicate-slide = Dupliquer la diapositive
deck-delete-slide = Supprimer la diapositive
deck-theme-label = Thème
deck-frame-comment = Commenter ce cadre
deck-add-text-frame = Ajouter un cadre de texte
deck-notes-drawer-label = Notes du présentateur
deck-present = Présenter
deck-present-empty = Cette présentation n’a pas encore de diapositives.
deck-present-next = Suiv.
deck-present-notes = Notes du présentateur
deck-present-follow = Suivre { $name }
deck-present-rejoin = Rejoindre le présentateur
sidebar-chats = Conversations
code-language-aria = Langage du bloc de code
code-language-plain = Texte brut
home-import-converting = Conversion de { $name }… Cela peut prendre quelques secondes.
home-bulk-delete-partial = { $succeeded } sur { $total } supprimés ; { $failed } échecs
home-import-too-large = { $name } fait { $size } octets ; la limite d’importation est de { $limit } octets.
home-import-unsupported = Type de fichier non pris en charge : { $name }. Utilisez .md, .markdown, .html, .htm, .docx ou .pdf.
home-import-read-failed = Impossible de lire { $name } : { $error }
home-import-invalid-text = { $name } ne contient pas de texte UTF-8 valide.
home-import-failed = Échec de l’importation : { $error }

ss-comment-replies =
    { $count ->
        [0] Aucune réponse
        [one] { $count } réponse
       *[other] { $count } réponses
    }

# Chat, history, and filter feedback
at-menu-ask-ai-query = Demander à l’IA : { $query }
chat-direct-default = Message direct ({ $count })
chat-group-default = Conversation ({ $count })
conversation-error-existing-thread = Ce bloc a déjà un commentaire. Sélectionnez la discussion existante pour répondre.
conversation-error-create-thread = Impossible de créer la discussion : { $error }
history-error-restore = Échec de la restauration : { $error }
notifications-on-document = dans { $title }
diff-image-placeholder = [image]
diff-image-description = [image : { $alt }]
kanban-assignee-placeholder = Nom d’utilisateur
ss-pivot-default-column = Colonne { $number }
ss-pivot-condition-in = dans [{ $values }]
ss-pivot-condition-contains = contient « { $text } »
ss-pivot-condition-starts = commence par « { $text } »
ss-pivot-condition-empty = vide
ss-pivot-condition-not-empty = non vide

# Paragraph base direction
menu-text-direction = Sens du texte
menu-direction-auto = Automatique
menu-direction-ltr = De gauche à droite
menu-direction-rtl = De droite à gauche
