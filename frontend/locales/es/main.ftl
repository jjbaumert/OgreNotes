# OgreNotes — Spanish (es) translations.
#
# Source-of-truth catalog is locales/en-US/main.ftl. Keys present
# in en-US but missing here fall back to en-US at runtime.

# ─── Common ─────────────────────────────────────────────────────

common-loading = Cargando…
common-send = Enviar
common-close = Cerrar
# Document Details panel (#141)
document-details-title = Detalles del documento
document-details-name = Nombre
document-details-type = Tipo
document-details-created = Creado
document-details-modified = Última modificación
document-details-words = Palabras
document-details-characters = Caracteres
doc-type-document = Documento
doc-type-spreadsheet = Hoja de cálculo
# Editor gutter (#139)
editor-page-break = Página { $n }
# Find & Replace bar (#147)
find-placeholder = Buscar
find-replace-placeholder = Reemplazar con
find-no-results = Sin resultados
find-prev = Coincidencia anterior
find-next = Coincidencia siguiente
find-replace = Reemplazar
find-replace-all = Reemplazar todo
common-delete = Eliminar
common-cancel = Cancelar
common-untitled = Sin título
common-redirecting-login = Redirigiendo al inicio de sesión…
common-redirecting = Redirigiendo…
common-open-navigation = Abrir navegación
common-restore-here = Restaurar aquí

# ─── Sidebar ────────────────────────────────────────────────────

sidebar-section-navigation = Navegación
# Favorites (#144)
document-favorite = Añadir a favoritos
# Expand / full screen (#145)
document-expand-enter = Expandir a pantalla completa
document-expand-exit = Salir de pantalla completa
document-unfavorite = Quitar de favoritos
sidebar-section-favorites = Favoritos
sidebar-empty-favorites = Aún no hay favoritos
sidebar-doc-open-new-tab = Abrir en una pestaña nueva
sidebar-doc-actions-aria = Acciones del documento
sidebar-home = Inicio
sidebar-search = Buscar
sidebar-sign-out = Cerrar sesión
sidebar-aria-main-nav = Navegación principal
sidebar-aria-collapse = Contraer barra lateral
sidebar-aria-expand = Expandir barra lateral

# ─── Menu bar ───────────────────────────────────────────────────

menubar-document = Documento
menubar-edit = Editar
menubar-view = Ver
menubar-insert = Insertar
menubar-format = Formato

# ─── Notification panel ─────────────────────────────────────────

notifications-title = Notificaciones
notifications-mark-all-read = Marcar todo como leído
notifications-empty = No hay notificaciones

# ─── Chat panel ─────────────────────────────────────────────────

chat-section-title = Chats
chat-empty = Aún no hay chats
chat-back = ← Volver a los chats
chat-message-placeholder = Escribe un mensaje…

# ─── Document outline ───────────────────────────────────────────

outline-title = Esquema
outline-empty = No hay encabezados
outline-aria-close = Cerrar esquema

# ─── Editor toolbar ─────────────────────────────────────────────

toolbar-undo = Deshacer (Ctrl+Z)
toolbar-redo = Rehacer (Ctrl+Shift+Z)
toolbar-bold = Negrita (Ctrl+B)
toolbar-italic = Cursiva (Ctrl+I)
toolbar-underline = Subrayado (Ctrl+U)
toolbar-strikethrough = Tachado
# Subscript / Superscript (#143)
toolbar-subscript = Subíndice
toolbar-superscript = Superíndice
toolbar-code = Código (Ctrl+E)
# Toolbar alignment controls (#134)
toolbar-align-left = Alinear a la izquierda
toolbar-align-center = Centrar
toolbar-align-right = Alinear a la derecha
toolbar-text-color = Color del texto
toolbar-remove-color = Quitar color
toolbar-highlight = Resaltar
toolbar-remove-highlight = Quitar resaltado
toolbar-image = Imagen
toolbar-link = Enlace (Ctrl+K)
toolbar-horizontal-rule = Línea horizontal
toolbar-insert-table = Insertar tabla
toolbar-insert-label = Insertar:
toolbar-comment = Comentar (Ctrl+Alt+C)
toolbar-comment-label = Comentar
toolbar-more = Más
toolbar-aria-more = Más opciones de la barra de herramientas
toolbar-prompt-url = Introduce la URL:

# Block-type dropdown
toolbar-block-paragraph = Párrafo
toolbar-block-heading-1 = Encabezado 1
toolbar-block-heading-2 = Encabezado 2
toolbar-block-heading-3 = Encabezado 3
toolbar-block-heading-4 = Encabezado 4
toolbar-block-bulleted-list = Lista con viñetas
toolbar-block-numbered-list = Lista numerada
toolbar-block-checklist = Lista de tareas
toolbar-block-blockquote = Cita
toolbar-block-code-block = Bloque de código
toolbar-block-format = Formato

# Number formats (spreadsheet block-type menu)
toolbar-num-general = General
toolbar-num-integer = Entero
toolbar-num-decimal-1 = Decimal (1)
toolbar-num-decimal-2 = Decimal (2)
toolbar-num-thousands = Miles
toolbar-num-currency-usd = Moneda (USD)
toolbar-num-currency-eur = Moneda (EUR)
toolbar-num-percent = Porcentaje

# ─── Comment popup ──────────────────────────────────────────────

comment-new-title = Nuevo comentario
comment-thread-title = Hilo de comentarios
comment-aria-prev = Comentario anterior
comment-aria-next = Comentario siguiente
comment-placeholder-new = Añade un comentario sobre esta sección
comment-placeholder-reply = Escribe un mensaje…

# ─── Conversation pane ──────────────────────────────────────────

conversation-thread = Hilo
conversation-comment-on-block = Comentar el bloque
conversation-comments = Comentarios
conversation-empty = Aún no hay comentarios. ¡Inicia una conversación!
conversation-placeholder-block = Comenta este bloque…
conversation-placeholder-add = Añade un comentario…
conversation-placeholder-reply = Responder…
conversation-aria-prev = Comentario anterior
conversation-aria-next = Comentario siguiente
conversation-back = ← Atrás
conversation-status-open = Abierto
conversation-status-resolved = Resuelto
conversation-resolve = Resolver
conversation-reopen = Reabrir
conversation-typing-1 = { $name } está escribiendo…
conversation-typing-2 = { $a } y { $b } están escribiendo…
conversation-typing-many = Varias personas están escribiendo…

# ─── History viewer ─────────────────────────────────────────────

history-title = Historial de edición
history-empty = Aún no hay historial de versiones
history-no-prior = No hay una versión anterior con la que comparar — esta es la primera instantánea.
history-changes-in-v = Cambios en la v{ $version }
history-restore-version = Restaurar versión
history-aria-close = Cerrar
history-jump-to-block-title = Saltar al bloque en el documento en vivo
history-jump-to-block-label = Saltar al bloque ↗
history-restoring = Restaurando…
history-restore-to-this-version = Restaurar a esta versión
history-restore-confirm-message = ¿Reemplazar el documento actual por esta versión? Se perderán los cambios locales no guardados.
history-restore-confirm-label = Restaurar
history-deleted-badge = (eliminado)

# Node-type labels for diff cards
node-paragraph = Párrafo
node-heading = Encabezado
node-bullet-list = Lista con viñetas
node-ordered-list = Lista numerada
node-list-item = Elemento de lista
node-task-list = Lista de tareas
node-task-item = Tarea
node-blockquote = Cita
node-code-block = Bloque de código
node-horizontal-rule = Separador
node-image = Imagen
node-table = Tabla
node-table-row = Fila de tabla
node-table-cell = Celda de tabla
node-table-header = Encabezado de tabla
node-block = Bloque

# ─── Login page ─────────────────────────────────────────────────

login-tagline = Documentos con garra.
login-error-name-email-required = Se requieren nombre y correo electrónico
login-placeholder-name = Nombre para mostrar
login-placeholder-email = Correo electrónico
login-signing-in = Iniciando sesión…
login-dev-button = Acceso de desarrollador (personalizado)
login-github = Iniciar sesión con GitHub
login-google = Iniciar sesión con Google

# ─── Share dialog ───────────────────────────────────────────────

share-title = Compartir
share-placeholder-email = Introduce una dirección de correo
share-button = Compartir
share-members-heading = Miembros actuales
share-role-owner = Propietario
share-role-edit = Puede editar
share-role-comment = Puede comentar
share-role-view = Puede ver
share-error-no-folder = El documento no tiene carpeta — no se puede compartir
share-error-enter-email = Introduce una dirección de correo
share-status-searching = Buscando…
share-error-search-failed = Búsqueda fallida: { $err }
share-error-no-user = No se encontró ningún usuario con el correo '{ $email }'
share-status-shared-with = Compartido con { $name }
share-error-failed = No se pudo compartir: { $err }

# Uso compartido por enlace (sección del diálogo de compartir, por documento)
share-link-heading = Uso compartido por enlace
share-link-mode-off = Desactivado
share-link-mode-view = Puede ver
share-link-mode-edit = Puede editar
share-link-note = Cualquier persona de tu espacio de trabajo con el enlace puede abrir esto.
share-link-off = El uso compartido por enlace está desactivado.
share-link-opt-comments = Permitir comentarios
share-link-opt-history = Mostrar historial de edición
share-link-opt-conversation = Mostrar conversación
share-link-opt-request = Permitir solicitudes de edición
share-link-copy = Copiar enlace
share-link-copied = Enlace copiado
share-link-saved = Guardado
share-link-error = No se pudo guardar: { $err }
# Viewer-facing request-edit-access affordance (#110)
share-link-request-banner = Estás viendo este documento a través de un enlace compartido.
share-link-request-button = Solicitar acceso de edición
share-link-request-sending = Enviando…
share-link-request-sent = Solicitud enviada
share-link-request-retry = No se pudo enviar: inténtalo de nuevo

# ─── Document page ──────────────────────────────────────────────

document-loading = Cargando documento…
document-trash-banner = Este documento está en la papelera — restáuralo para editar.
document-trash-restore = Restaurar
document-trash-delete-forever = Eliminar para siempre
document-share-tooltip = Compartir
# Document menu: rename + move (#146)
document-rename-prompt = Cambiar el nombre del documento
document-move-folder-title = Mover a una carpeta
document-move-here = Mover aquí
# Duplicate dialog (#146)
duplicate-dialog-title = Duplicar documento
duplicate-name-label = Nombre
duplicate-destination-label = Carpeta de destino
duplicate-confirm = Duplicar
duplicate-share-warning = Esta carpeta está compartida: { $count } personas más tienen acceso y verán la copia.
# Focus/expand toggle (#134)
document-focus-enter = Modo concentración
document-focus-exit = Salir del modo concentración
document-trash-dialog-title = Mover a la papelera
document-trash-dialog-message = Este documento se moverá a la papelera. Podrás restaurarlo más tarde.
document-trash-dialog-confirm = Mover a la papelera
document-purge-dialog-title = ¿Eliminar para siempre?
document-purge-dialog-message = Esto elimina permanentemente el documento y todo su contenido. No se puede deshacer.
document-purge-dialog-confirm = Eliminar para siempre
document-restore-folder-title = Restaurar a una carpeta

# ─── Home page ──────────────────────────────────────────────────

home-new-document = + Nuevo documento
home-new-spreadsheet = + Nueva hoja de cálculo
home-new-folder = + Nueva carpeta

# ─── MFA (enroll + challenge) ───────────────────────────────────

mfa-verifying = Verificando…
mfa-enter-totp = Introduce el código de 6 dígitos de tu autenticador
mfa-enter-recovery = Introduce tu código de recuperación

mfa-enroll-title = Configurar la autenticación de dos factores
mfa-enroll-subtitle = Escanea el código QR con tu aplicación de autenticación e introduce el código de 6 dígitos para confirmar.
mfa-enroll-success = Inscripción confirmada. Redirigiendo…
mfa-enroll-error-failed = Error al inscribirse: { $err }
mfa-enroll-error-verify-failed = Error al verificar: { $err }
mfa-enroll-manual-entry = Entrada manual
mfa-enroll-recovery-codes-summary = Códigos de recuperación (¡guárdalos ahora!)
mfa-enroll-recovery-warning = Cada código puede usarse una sola vez si pierdes el acceso a tu autenticador. No volveremos a mostrarlos.
mfa-enroll-code-label = Código del autenticador
mfa-enroll-confirm = Confirmar

mfa-challenge-title = Autenticación de dos factores
mfa-challenge-subtitle-totp = Abre tu aplicación de autenticación e introduce el código de 6 dígitos.
mfa-challenge-subtitle-recovery = Introduce uno de tus códigos de recuperación de un solo uso.
mfa-challenge-verify = Verificar
mfa-challenge-missing-handle = Falta el identificador MFA, redirigiendo al inicio de sesión…
mfa-challenge-error-invalid-totp = Código no válido — comprueba tu autenticador e inténtalo de nuevo
mfa-challenge-error-invalid-recovery = Código de recuperación no válido — cada código solo puede usarse una vez
mfa-challenge-use-totp = Usar el código del autenticador en su lugar
mfa-challenge-use-recovery = ¿Perdiste tu autenticador? Usa un código de recuperación

# ─── Admin console (platform-admin pages) ───────────────────────

admin-loading = Cargando consola de administración…
admin-redirecting = Redirigiendo…
admin-status-active = activo
admin-status-disabled = deshabilitado
admin-status-never = nunca
admin-role-admin = administrador
admin-role-user = usuario
admin-retry = Reintentar

# Admin sub-nav
admin-nav-users = Usuarios
admin-nav-metrics = Métricas
admin-nav-audit = Auditoría
admin-nav-back = Volver a la aplicación

# Admin · Users
admin-users-title = Admin · Usuarios
admin-users-search-placeholder = Filtrar por prefijo de correo
admin-users-th-email = Correo
admin-users-th-name = Nombre
admin-users-th-role = Rol
admin-users-th-state = Estado
admin-users-th-last-active = Última actividad
admin-users-th-actions = Acciones
admin-users-enable = Habilitar
admin-users-disable = Deshabilitar
admin-users-promote = Promover
admin-users-demote = Degradar
admin-users-prev = Anterior
admin-users-next = Siguiente
admin-users-error-list-failed = Error al listar: { $err }
admin-users-error-action-failed = { $action } falló: { $err }

# Admin · Audit
admin-audit-title = Admin · Registro de auditoría
admin-audit-label-target = ID del usuario objetivo
admin-audit-label-actor = ID del actor
admin-audit-label-kind = Tipo
admin-audit-placeholder-kind = p. ej. disable, loginFailure
admin-audit-label-from = Desde (ISO)
admin-audit-label-to = Hasta (ISO)
admin-audit-search = Buscar
admin-audit-error-target-required = Se requiere el ID del usuario objetivo
admin-audit-error-load-failed = Error al cargar: { $err }
admin-audit-th-when = Cuándo
admin-audit-th-source = Origen
admin-audit-th-kind = Tipo
admin-audit-th-actor = Actor
admin-audit-th-target = Objetivo
admin-audit-th-detail = Detalle

# Admin · Metrics
admin-metrics-title = Admin · Métricas
admin-metrics-refresh = Actualizar
admin-metrics-error-fetch-failed = Error al obtener: { $err }
admin-metrics-counters = Contadores
admin-metrics-gauges = Indicadores
admin-metrics-histograms = Histogramas
admin-metrics-th-key = Clave
admin-metrics-th-value = Valor
admin-metrics-th-count = Recuento
admin-metrics-th-sum = Suma
admin-metrics-th-min = Mín.
admin-metrics-th-max = Máx.

# ─── Workspace SCIM tokens ──────────────────────────────────────

scim-title = Tokens SCIM del espacio de trabajo
scim-subtitle = Crea un token de portador para el conector de aprovisionamiento SCIM de tu IdP. El texto en claro se muestra una sola vez al crearlo — cópialo de inmediato.
scim-base-url-heading = URL base SCIM
scim-base-url-help = Pega esto en la configuración del conector SCIM de tu IdP.
scim-fresh-heading = Nuevo token: { $name }
scim-fresh-warning = Copia este token AHORA — no se mostrará de nuevo.
scim-fresh-copy = Copiar
scim-create-heading = Crear un nuevo token
scim-create-placeholder = Etiqueta (p. ej., conector Okta)
scim-create-button = Crear
scim-existing-heading = Tokens existentes
scim-empty = Aún no hay tokens. Crea uno arriba.
scim-th-name = Nombre
scim-th-token-id = ID del token
scim-th-created = Creado
scim-th-last-used = Último uso
scim-th-status = Estado
scim-status-active = activo
scim-status-revoked = revocado
scim-revoke = Revocar
scim-error-name-required = Se requiere el nombre del token
scim-error-load-failed = Error al cargar: { $err }
scim-error-create-failed = Error al crear: { $err }
scim-error-revoke-failed = Error al revocar: { $err }

# ─── Workspace SAML SSO ─────────────────────────────────────────

saml-title = SSO SAML del espacio de trabajo
saml-subtitle-prefix = Configura un IdP SAML 2.0 para este espacio de trabajo. Los miembros podrán iniciar sesión a través del IdP en
saml-subtitle-suffix = una vez que guardes.
saml-status-saved = Configuración SAML guardada.
saml-status-removed = Configuración SAML eliminada.
saml-status-copied = URL de metadatos SP copiada al portapapeles.
saml-sp-heading = Metadatos SP
saml-sp-help = Copia esta URL en el flujo «añadir proveedor de servicios» de tu IdP. O recupera la URL una vez y pega el XML de respuesta en tu IdP.
saml-copy = Copiar
saml-idp-heading = Configuración del IdP
saml-idp-help = Pega el XML de metadatos que te proporcione tu IdP. Se requiere el XML completo — incluido el elemento raíz <EntityDescriptor>.
saml-label-entity-id = Entity ID del IdP
saml-placeholder-entity-id = https://idp.example.com/metadata
saml-label-metadata-xml = XML de metadatos del IdP
saml-label-email-attr = Nombre del atributo de correo
saml-label-name-attr = Nombre del atributo de nombre
saml-save = Guardar
saml-update = Actualizar
saml-remove = Eliminar
saml-error-entity-id-required = Se requiere el Entity ID del IdP
saml-error-metadata-required = Se requiere el XML de metadatos del IdP
saml-error-load-failed = Error al cargar: { $err }
saml-error-save-failed = Error al guardar: { $err }
saml-error-delete-failed = Error al eliminar: { $err }
saml-meta-first-configured = Configurado por primera vez
saml-meta-last-updated = ; última actualización

# ─── Spreadsheet view (chrome) ──────────────────────────────────

ss-empty = Sin datos
ss-format-painter-title = Copiar formato — haz clic para copiar el formato de la celda activa y luego haz clic en un destino. Mayús+clic para modo continuo.
ss-format-painter-status = Copiando formato — haz clic en una celda para aplicar, Esc para cancelar
ss-format-painter-status-sticky = Copiando formato (continuo) — haz clic en celdas para aplicar, Esc para detener
ss-sort-tooltip = Ordenar la hoja de cálculo…

# Status bar
ss-status-count = Recuento: { $value }
ss-status-sum = Suma: { $value }
ss-status-avg = Media: { $value }
ss-status-min = Mín.: { $value }
ss-status-max = Máx.: { $value }

# Sheet tabs
ss-rename-sheet-prompt = Renombrar hoja:
ss-touch-menu-aria = Acciones de celda
ss-ctx-rename = Renombrar
ss-ctx-delete = Eliminar

# Find / replace bar
ss-find-placeholder = Buscar…
ss-replace-placeholder = Reemplazar…
ss-find-next = Siguiente
ss-find-replace = Reemplazar
ss-find-replace-all = Reemplazar todo
ss-find-no-results = 0 resultados

# Filter dropdown
ss-filter-header = Filtrar: { $col }
ss-filter-show-all = Mostrar todo
ss-filter-custom-prompt = Filtro personalizado (p. ej. >100, <0, =Done, contains:err, empty, notempty):
ss-filter-custom-button = Filtro personalizado…
ss-filter-empty-value = (vacío)

# Sort dialog
ss-sort-title = Ordenar
ss-sort-range-label = Rango:
ss-sort-has-headers = La primera fila contiene encabezados (omitir al ordenar)
ss-sort-by-label = Ordenar por
ss-sort-then-by-label = Después por
ss-sort-asc = Ascendente
ss-sort-desc = Descendente
ss-sort-remove-level-title = Quitar este nivel de orden
ss-sort-add-level = + Añadir nivel de orden
ss-sort-cancel = Cancelar
ss-sort-apply = Aplicar
ss-sort-err-parse-range = No se pudo interpretar el rango. Usa notación A1, p. ej. A1:G41.
ss-sort-err-no-keys = Añade al menos una clave de ordenación.

# Foreign-document consent dialog
ss-foreign-title = Este documento obtiene datos de otros libros
ss-foreign-hint = Permitir las consultas usa el acceso de lectura de tu cuenta a esos documentos. La aprobación dura solo esta sesión.
ss-foreign-deny = Denegar
ss-foreign-allow = Permitir

# ─── Spreadsheet context menu (cell right-click) ────────────────

ss-ctx-menu-insert = Insertar
ss-ctx-menu-delete = Eliminar
ss-ctx-menu-sort = Ordenar
ss-ctx-menu-format = Formato
ss-ctx-menu-comment = Comentario
ss-ctx-menu-hide = Ocultar / Mostrar
ss-ctx-menu-data = Datos
ss-ctx-menu-cond-fmt = Formato condicional
ss-ctx-menu-validation = Validación de datos
ss-ctx-insert-row-above = Insertar fila encima
ss-ctx-insert-row-below = Insertar fila debajo
ss-ctx-insert-col-left = Insertar columna a la izquierda
ss-ctx-insert-col-right = Insertar columna a la derecha
ss-ctx-sort-dialog = Ordenar…
ss-ctx-delete-row = Eliminar fila
ss-ctx-delete-rows = Eliminar { $count } filas
ss-ctx-delete-col = Eliminar columna
ss-ctx-delete-cols = Eliminar { $count } columnas
ss-ctx-clear-contents = Borrar contenido
ss-ctx-sort-a-z = Ordenar A → Z
ss-ctx-sort-z-a = Ordenar Z → A
ss-ctx-freeze-rows = Inmovilizar filas superiores
ss-ctx-unfreeze-rows = Liberar filas
ss-ctx-freeze-cols = Inmovilizar columnas a la izquierda
ss-ctx-unfreeze-cols = Liberar columnas

# Cell validation
ss-ctx-set-checkbox = Establecer como casilla
ss-ctx-set-dropdown = Establecer como lista desplegable…
ss-ctx-remove-validation = Quitar validación
ss-ctx-dropdown-prompt = Introduce las opciones del desplegable (separadas por comas):

# Conditional formatting
ss-ctx-cond-fmt = Formato condicional…
ss-ctx-cond-fmt-prompt = Formato condicional (p. ej., >100, <0, =Done, contains:error, empty, notempty):
ss-ctx-cond-fmt-color-prompt = Color de fondo (p. ej., #ff0000, red, #ffd):
ss-ctx-color-scale = Escala de color…
ss-ctx-color-scale-prompt = Escala de color: bajo,alto o bajo,medio,alto (p. ej. #ff0000,#ffff00,#00ff00):
ss-ctx-data-bar = Barra de datos…
ss-ctx-data-bar-prompt = Color de la barra de datos:
ss-ctx-icon-set = Conjunto de iconos…
ss-ctx-icon-set-prompt = Conjunto de iconos: arrows o traffic

# Charts + pivots
ss-ctx-insert-chart = Insertar gráfico…
ss-ctx-chart-type-prompt = Tipo de gráfico (bar, line, pie):
ss-ctx-chart-title-prompt = Título del gráfico:
ss-ctx-chart-unknown-type = Tipo de gráfico desconocido. Usa uno de: bar, line, pie.
ss-ctx-insert-pivot = Insertar tabla dinámica…
ss-ctx-pivot-needs-multi = La tabla dinámica necesita una selección de varias filas y columnas. Selecciona tus datos (con la fila de encabezado en la fila 1) e inténtalo de nuevo.

# CSV import + merge
ss-ctx-import-csv = Importar CSV…
ss-ctx-merge-cells = Combinar celdas
ss-ctx-unmerge-cells = Separar celdas

# Hide / unhide
ss-ctx-hide-row = Ocultar fila
ss-ctx-unhide-all-rows = Mostrar todas las filas
ss-ctx-hide-col = Ocultar columna
ss-ctx-unhide-all-cols = Mostrar todas las columnas

# Cell lock + comments + named ranges
ss-ctx-lock-cell = Bloquear celda
ss-ctx-unlock-cell = Desbloquear celda
ss-ctx-add-comment = Añadir comentario…
ss-ctx-edit-comment = Editar comentario…
ss-ctx-open-comment = Abrir hilo de comentarios…
ss-ctx-comment-prompt = Comentario:
ss-ctx-remove-comment = Quitar comentario
ss-comment-preview-empty = Aún no hay mensajes
ss-comment-replies-none = Sin respuestas
ss-comment-replies-one = 1 respuesta
ss-comment-replies-many = { $count } respuestas
ss-ctx-define-name = Definir nombre…
ss-ctx-name-prompt = Nombre para este rango:
ss-ctx-remove-name = Quitar nombre…
ss-ctx-no-named-ranges = No hay rangos con nombre definidos.
ss-ctx-remove-name-prompt = ¿Qué nombre quitar? Definidos: { $names }

# ─── Pivot table editor ─────────────────────────────────────────

ss-pivot-title = Editor de tabla dinámica
ss-pivot-foreign-source-label = Origen externo:
ss-pivot-foreign-hint = Aún no se admite la edición de origen externo. Edita la configuración de la tabla dinámica mediante el atributo JSON o elimínala y vuelve a crearla como dinámica local.
ss-pivot-layout = Diseño
ss-pivot-layout-compact = Compacto
ss-pivot-layout-outline = Esquema
ss-pivot-layout-tabular = Tabular
ss-pivot-totals = Totales
ss-pivot-totals-none = Ninguno
ss-pivot-totals-rows = Filas
ss-pivot-totals-cols = Cols
ss-pivot-totals-both = Ambos
ss-pivot-edit-filter-tooltip = Editar filtro
ss-pivot-axis-row = Fila
ss-pivot-axis-col = Columna
ss-pivot-labels-header = Etiquetas { $axis } — { $col }
ss-pivot-close-tooltip = Cerrar
ss-pivot-close-editor-tooltip = Cerrar editor (la tabla dinámica sigue mostrándose)
ss-pivot-filter-all = Todos
ss-pivot-filter-none = Ninguno
ss-pivot-filter-prefix = Filtrar { $col }
ss-pivot-source-label = Origen:
ss-pivot-delete = Eliminar tabla dinámica
ss-pivot-section-fields = Campos
ss-pivot-section-rows = Filas
ss-pivot-section-cols = Columnas
ss-pivot-section-values = Valores
ss-pivot-section-filters = Filtros
ss-pivot-search-placeholder = Buscar campos…
ss-pivot-bin-width-tooltip = Ancho del intervalo

# Date granularity options
ss-pivot-date-year = Año
ss-pivot-date-quarter = Trimestre
ss-pivot-date-month = Mes
ss-pivot-date-day = Día
ss-pivot-date-hour = Hora

# ─── App-level / router ─────────────────────────────────────────

app-not-found = Página no encontrada

# ─── Accessibility ──────────────────────────────────────────────

a11y-skip-to-content = Saltar al contenido principal
a11y-toolbar-label = Barra de formato del documento
a11y-toolbar-group-undo = Deshacer y rehacer
a11y-toolbar-group-block-type = Tipo de bloque
a11y-toolbar-group-inline = Formato en línea
a11y-toolbar-group-align = Alineación
a11y-toolbar-group-block = Formato de bloque
a11y-toolbar-group-insert = Insertar
a11y-file-table-label = Documentos y carpetas
a11y-breadcrumb-label = Ruta de carpetas

# ─── @-menu (mention picker) ────────────────────────────────────

at-menu-empty = Escribe para buscar personas y documentos…

# ─── File browser (home page table) ─────────────────────────────

file-browser-empty = Aquí no hay nada todavía. Crea un documento o una carpeta.
file-browser-th-title = Título
file-browser-th-added = Añadido
file-type-folder = Carpeta
file-type-document = Documento
file-type-spreadsheet = Hoja de cálculo
file-type-chat = Chat

# ─── Folder picker ──────────────────────────────────────────────

folder-picker-not-available =  (no disponible)

# ─── Formula keyboard ───────────────────────────────────────────

formula-key-backspace = Retroceso
formula-key-cancel = Cancelar (Esc)
formula-key-commit = Confirmar (Entrar)
# Mode-switcher tabs (Phase 5 M-P3 piece C).
kbd-mode-standard = Aa
kbd-mode-numeric = 123
kbd-mode-formula = ƒx
kbd-standard-hint = Usa el teclado de tu dispositivo

# ─── Search dialog ──────────────────────────────────────────────

search-placeholder = Buscar documentos…
search-searching = Buscando…
search-no-results = No se encontraron resultados
search-dialog-label = Buscar documentos o ejecutar comandos

# ─── Ask (asistente RAG) ────────────────────────────────────────

ask-dialog-title = Preguntar al asistente
ask-badge = IA
ask-placeholder = Haz una pregunta sobre tus documentos…
ask-empty-hint = El asistente busca en tus documentos y cita lo que encuentre. Pregunta algo concreto.
ask-sources-heading = Fuentes
ask-error-rate-limit = Demasiadas solicitudes. Espera un momento y vuelve a intentarlo.
ask-error-disabled = Un administrador ha deshabilitado el asistente para tu espacio de trabajo.
ask-error-unavailable = El asistente no está disponible temporalmente.
sidebar-ask = Preguntar

# ─── Relationships ──────────────────────────────────────────────

relationship-heading = Relacionado
relationship-empty = Aún no hay documentos relacionados.
relationship-add-aria = Añadir un documento relacionado
relationship-remove-aria = Quitar esta relación
relationship-picker-placeholder = Buscar documentos para enlazar…
relationship-picker-aria = Buscar documentos para enlazar
relationship-picker-confirm = Enlazar
relationship-type-aria = Tipo de relación
relationship-error-self = Un documento no puede enlazarse consigo mismo.
relation-type-implements = Implementa
relation-type-derived-from = Derivado de
relation-type-depends-on = Depende de
relation-type-references = Referencia a
relation-type-supersedes = Reemplaza a

# ─── Theme selector ─────────────────────────────────────────────

theme-aria-label = Tema
theme-system = Seguir el tema del sistema
theme-light = Tema claro
theme-dark = Tema oscuro

# ─── Locale selector ────────────────────────────────────────────

locale-aria-label = Idioma

# ─── Inline selection / comment-bubble ──────────────────────────

selection-toolbar-comment = Comentar la selección
comment-highlights-add = Añadir comentario

# ─── Auth-callback page ─────────────────────────────────────────

auth-complete-status = Completando el inicio de sesión…

# ─── Sync indicator (Phase 5 M-P3 piece B) ──────────────────────

sync-saved = Guardado
sync-saving = Guardando…
sync-offline = Sin conexión
sync-offline-pending = Sin conexión — {$count} pendientes
sync-saved-tooltip = Tus cambios están guardados.
sync-saving-tooltip = Enviando tus últimos cambios al servidor…
sync-offline-tooltip = Estás desconectado. Vuelve a conectarte para seguir colaborando.
sync-offline-pending-tooltip = Estás desconectado. {$count} cambio(s) aún no han llegado al servidor.

# ─── Command palette (Phase 5 M-P4 piece A) ─────────────────────

palette-no-actions = No hay comandos que coincidan.
cmd-go-home = Ir al inicio
cmd-toggle-dark-mode = Alternar modo oscuro
cmd-open-trash = Abrir papelera
cmd-sign-out = Cerrar sesión
cmd-ask = Preguntar al asistente
cmd-about-palette = Paleta de comandos: acerca de
# Editor-scoped commands (M-P4 piece B).
cmd-bold = Negrita
cmd-italic = Cursiva
cmd-underline = Subrayado
cmd-strike = Tachado
cmd-code = Código en línea
cmd-heading-1 = Encabezado 1
cmd-heading-2 = Encabezado 2
cmd-heading-3 = Encabezado 3
cmd-paragraph = Párrafo
cmd-bullet-list = Lista con viñetas
cmd-ordered-list = Lista numerada
cmd-task-list = Lista de tareas
cmd-blockquote = Cita
cmd-code-block = Bloque de código
cmd-divider = Insertar separador
cmd-insert-table = Insertar tabla
cmd-undo = Deshacer
cmd-redo = Rehacer

# ─── Home-page drop-to-import (Phase 5 M-P5 piece D) ────────────

home-drop-title = Suelta para importar
home-drop-hint = Markdown (.md) o HTML (.html) — hasta 1 MB
home-import-default-title = Importado

# ─── Toolbar — embed insert (Phase 5 M-P6 piece B) ──────────────

toolbar-embed = Insertar medio (URL)

# ─── File-browser bulk selection (Phase 5 M-P7 piece C) ─────────

file-browser-th-select = Seleccionar
bulk-selection-count = {$count} seleccionados
bulk-selection-cancel = Cancelar
bulk-selection-delete = Eliminar
bulk-delete-dialog-title = ¿Mover los documentos seleccionados a la papelera?
bulk-delete-dialog-message = Los documentos seleccionados se moverán a tu papelera. Podrás restaurarlos en un plazo de 30 días.
bulk-delete-dialog-confirm = Mover a la papelera

# ─── Account settings page (design/account-menu.md, step 1) ─────

settings-title = Ajustes
settings-aria-tabs = Secciones de ajustes
settings-tab-profile = Perfil
settings-tab-appearance = Apariencia
settings-tab-notifications = Notificaciones
settings-tab-accessibility = Accesibilidad
settings-tab-help = Ayuda y soporte
settings-coming-soon = Esta sección estará disponible próximamente.

# Account menu & settings (account-menu feature)
account-menu-aria = Menú de cuenta
account-menu-profile = Perfil y estado
account-menu-settings = Ajustes
account-menu-shortcuts = Atajos de teclado
settings-a11y-dyslexic-label = Fuente para dislexia
settings-a11y-dyslexic-hint = Usa una tipografía más legible para el texto de los documentos.
settings-a11y-reduce-motion-label = Reducir movimiento
settings-a11y-reduce-motion-hint = Minimiza las animaciones y transiciones en toda la aplicación.
settings-appearance-language = Idioma
# BYOK — bring-your-own Anthropic key (#29)
settings-byok-label = Asistente de IA: usa tu propia clave de Anthropic
settings-byok-hint = Se guarda solo en este navegador y se envía con tus solicitudes de IA; nunca se almacena en nuestros servidores. Déjalo en blanco para usar la clave del espacio de trabajo.
settings-byok-active = Usando tu clave
settings-byok-none = Usando la clave del espacio de trabajo.
settings-byok-save = Guardar clave
settings-byok-clear = Quitar clave
settings-appearance-theme = Tema
settings-help-shortcuts = Atajos de teclado
settings-help-shortcut-palette = Abrir paleta de comandos / búsqueda
settings-help-shortcut-actions = Paleta de comandos (acciones)
settings-help-version = Versión
settings-notif-email-heading = Notificaciones por correo
settings-notif-all = Toda la actividad
settings-notif-mentions = Solo menciones
settings-notif-off = Desactivadas
settings-notif-hint = Controla qué actividad te envía correos. Las notificaciones en la app no se ven afectadas.
settings-profile-name = Nombre visible
settings-profile-avatar = URL del avatar
settings-profile-email = Correo electrónico
settings-profile-email-hint = Tu correo lo gestiona tu proveedor de inicio de sesión y no se puede cambiar aquí.
settings-save = Guardar cambios
settings-saving = Guardando…
settings-saved = Guardado
settings-profile-error = No se pudieron guardar los cambios. Inténtalo de nuevo.
settings-profile-load-error = No se pudo cargar tu perfil. Vuelve a cargar la página para intentarlo de nuevo.
settings-profile-name-required = El nombre visible no puede estar vacío.
settings-profile-avatar-invalid = La URL del avatar debe empezar por http:// o https://.
settings-status-heading = Estado
settings-status-emoji = Emoji de estado
settings-status-text = ¿Cuál es tu estado?
settings-status-expiry = Borrar después de
settings-status-expiry-never = No borrar
settings-status-expiry-30m = 30 minutos
settings-status-expiry-1h = 1 hora
settings-status-expiry-4h = 4 horas
settings-status-set = Establecer estado
settings-status-clear = Borrar estado
theme-label-system = Sistema
theme-label-light = Claro
theme-label-dark = Oscuro

# --- menu bar + editor context menu (i18n backfill) ---
menu-cut = Cortar
menu-copy = Copiar
menu-paste = Pegar
menu-bold = Negrita
menu-italic = Cursiva
menu-underline = Subrayado
menu-strikethrough = Tachado
menu-code = Código
menu-comment = Comentar
menu-alignment = Alineación
menu-align-left = Izquierda
menu-align-center = Centro
menu-align-right = Derecha
menubar-doc-new = Nuevo
menubar-doc-share = Compartir…
menubar-doc-copy-link = Copiar enlace
menubar-doc-move-folder = Mover a carpeta…
menubar-doc-duplicate = Duplicar…
menubar-doc-new-template = Nuevo desde plantilla…
menubar-doc-export = Exportar
menubar-doc-export-html = HTML
menubar-doc-export-markdown = Markdown (copiar)
menubar-doc-export-csv = CSV
menubar-doc-export-excel = Excel (.xlsx)
menubar-doc-print = Imprimir…
menubar-doc-history = Historial del documento…
menubar-doc-details = Detalles del documento…
menubar-doc-rename = Renombrar documento…
menubar-doc-delete = Eliminar documento…
menubar-edit-undo = Deshacer
menubar-edit-redo = Rehacer
menubar-edit-find = Buscar y reemplazar
menubar-view-comments = Mostrar comentarios
menubar-view-conversation = Mostrar conversación
menubar-view-cursors = Mostrar cursores
menubar-view-focus = Modo concentración
menubar-view-line-numbers = Mostrar números de línea
menubar-view-page-breaks = Mostrar saltos de página
menubar-view-outline = Mostrar esquema
menubar-format-subscript = Subíndice
menubar-format-superscript = Superíndice
menubar-format-paragraph-style = Estilo de párrafo
menubar-format-list = Lista
menubar-format-clear = Borrar formato
menubar-format-lock = Bloquear ediciones
editorctx-paragraph-style = Estilo de párrafo
editorctx-insert-link = Insertar enlace…
