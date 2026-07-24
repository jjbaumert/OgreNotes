# OgreNotes — Arabic translations (RTL pilot).
#
# Source-of-truth catalog is locales/en-US/main.ftl. Keys present
# in en-US but missing here fall back to en-US at runtime — the
# fallback is reported in debug builds so missing-translation gaps
# are visible without panicking.
#
# Arabic is the M-P2 v1 RTL pilot. The harness's RTL behavior
# (flipping <html dir="rtl">, CSS logical properties picking up
# the swap) is exercised by switching to this locale.

# ─── Common ─────────────────────────────────────────────────────

common-loading = جارٍ التحميل…
common-send = إرسال
common-close = إغلاق
# Document Details panel (#141)
document-details-title = تفاصيل المستند
document-details-name = الاسم
document-details-type = النوع
document-details-created = تاريخ الإنشاء
document-details-modified = آخر تعديل
document-details-words = الكلمات
document-details-characters = الأحرف
doc-type-document = مستند
doc-type-spreadsheet = جدول بيانات
# Editor gutter (#139)
editor-page-break = صفحة { $n }
# Find & Replace bar (#147)
find-placeholder = بحث
find-replace-placeholder = استبدال بـ
find-no-results = لا نتائج
find-prev = التطابق السابق
find-next = التطابق التالي
find-replace = استبدال
find-replace-all = استبدال الكل
common-delete = حذف
common-cancel = إلغاء
common-untitled = بدون عنوان
common-redirecting-login = إعادة التوجيه إلى تسجيل الدخول…
common-open-navigation = فتح التنقل
common-restore-here = استعادة هنا

# ─── Sidebar ────────────────────────────────────────────────────

sidebar-section-navigation = التنقل
# Favorites (#144)
document-favorite = إضافة إلى المفضلة
# Expand / full screen (#145)
document-expand-enter = توسيع إلى ملء الشاشة
document-expand-exit = الخروج من ملء الشاشة
document-unfavorite = إزالة من المفضلة
sidebar-section-favorites = المفضلة
sidebar-empty-favorites = لا توجد عناصر مفضلة بعد
sidebar-doc-open-new-tab = فتح في علامة تبويب جديدة
sidebar-doc-actions-aria = إجراءات المستند
sidebar-new-aria = إنشاء جديد
sidebar-new-document = مستند جديد
sidebar-new-spreadsheet = جدول بيانات جديد
menubar-help = مساعدة
sidebar-home = الرئيسية
sidebar-search = بحث
sidebar-sign-out = تسجيل الخروج
sidebar-aria-main-nav = التنقل الرئيسي
sidebar-aria-collapse = طي الشريط الجانبي
sidebar-aria-expand = توسيع الشريط الجانبي

# ─── Menu bar ───────────────────────────────────────────────────

menubar-document = مستند
menubar-edit = تحرير
menubar-view = عرض
menubar-insert = إدراج
menubar-format = تنسيق

# ─── Notification panel ─────────────────────────────────────────

notifications-title = الإشعارات
notifications-mark-all-read = تعليم الكل كمقروء
notifications-empty = لا توجد إشعارات

# ─── Chat panel ─────────────────────────────────────────────────

chat-section-title = المحادثات
chat-empty = لا توجد محادثات بعد
chat-back = ← العودة إلى المحادثات
chat-message-placeholder = اكتب رسالة…

# ─── Document outline ───────────────────────────────────────────

outline-title = المخطط
outline-empty = لا توجد عناوين
outline-aria-close = إغلاق المخطط

# ─── Editor toolbar ─────────────────────────────────────────────

toolbar-undo = تراجع (Ctrl+Z)
toolbar-redo = إعادة (Ctrl+Shift+Z)
toolbar-bold = عريض (Ctrl+B)
toolbar-italic = مائل (Ctrl+I)
toolbar-underline = تسطير (Ctrl+U)
toolbar-strikethrough = يتوسطه خط
# Subscript / Superscript (#143)
toolbar-subscript = منخفض
toolbar-superscript = مرتفع
toolbar-code = شفرة (Ctrl+E)
# Toolbar alignment controls (#134)
toolbar-align-left = محاذاة لليسار
toolbar-align-center = توسيط
toolbar-align-right = محاذاة لليمين
toolbar-text-color = لون النص
toolbar-remove-color = إزالة اللون
toolbar-highlight = تمييز
toolbar-remove-highlight = إزالة التمييز
toolbar-image = صورة
toolbar-link = رابط (Ctrl+K)
toolbar-horizontal-rule = خط أفقي
toolbar-insert-table = إدراج جدول
toolbar-insert-label = إدراج:
toolbar-comment = تعليق (Ctrl+Alt+C)
toolbar-comment-label = تعليق
toolbar-more = المزيد
toolbar-aria-more = خيارات شريط الأدوات الإضافية
toolbar-prompt-url = أدخل عنوان URL:

# Block-type dropdown
toolbar-block-paragraph = فقرة
toolbar-block-heading-1 = عنوان 1
toolbar-block-heading-2 = عنوان 2
toolbar-block-heading-3 = عنوان 3
toolbar-block-heading-4 = عنوان 4
toolbar-block-bulleted-list = قائمة نقطية
toolbar-block-numbered-list = قائمة مرقمة
toolbar-block-checklist = قائمة مراجعة
toolbar-block-blockquote = اقتباس
toolbar-block-code-block = كتلة شفرة
toolbar-block-format = تنسيق

# Number formats (spreadsheet block-type menu)
toolbar-num-general = عام
toolbar-num-integer = عدد صحيح
toolbar-num-decimal-1 = عشري (1)
toolbar-num-decimal-2 = عشري (2)
toolbar-num-thousands = آلاف
toolbar-num-currency-usd = عملة (دولار أمريكي)
toolbar-num-currency-eur = عملة (يورو)
toolbar-num-percent = نسبة مئوية

# ─── Comment popup ──────────────────────────────────────────────

comment-new-title = تعليق جديد
comment-thread-title = سلسلة التعليقات
comment-aria-prev = التعليق السابق
comment-aria-next = التعليق التالي
comment-placeholder-new = أضف تعليقًا حول هذا القسم
comment-placeholder-reply = اكتب رسالة…

# ─── Conversation pane ──────────────────────────────────────────

conversation-thread = سلسلة محادثات
conversation-comment-on-block = تعليق على الكتلة
conversation-comments = تعليقات
conversation-empty = لا توجد تعليقات بعد. ابدأ محادثة!
conversation-placeholder-block = علِّق على هذه الكتلة…
conversation-placeholder-add = أضف تعليقًا…
conversation-placeholder-reply = ردّ…
conversation-aria-prev = التعليق السابق
conversation-aria-next = التعليق التالي
conversation-back = → رجوع
conversation-status-open = مفتوح
conversation-status-resolved = محلول
conversation-resolve = حل
conversation-reopen = إعادة فتح
conversation-typing-1 = { $name } يكتب…
conversation-typing-2 = { $a } و { $b } يكتبان…
conversation-typing-many = عدة أشخاص يكتبون…

# ─── History viewer ─────────────────────────────────────────────

history-title = سجل التحرير
history-empty = لا يوجد سجل إصدارات بعد
history-no-prior = لا توجد نسخة سابقة للمقارنة — هذه أول لقطة.
history-changes-in-v = التغييرات في الإصدار { $version }
history-restore-version = استعادة الإصدار
history-aria-close = إغلاق
history-jump-to-block-title = الانتقال إلى الكتلة في المستند المباشر
history-jump-to-block-label = الانتقال إلى الكتلة ↗
history-restoring = جارٍ الاستعادة…
history-restore-to-this-version = استعادة إلى هذا الإصدار
history-restore-confirm-message = هل تريد استبدال المستند الحالي بهذه النسخة؟ ستفقد أي تغييرات محلية غير محفوظة.
history-restore-confirm-label = استعادة
history-deleted-badge = (محذوف)

# Node-type labels for diff cards
node-paragraph = فقرة
node-heading = عنوان
node-bullet-list = قائمة نقطية
node-ordered-list = قائمة مرقمة
node-list-item = عنصر قائمة
node-task-list = قائمة مهام
node-task-item = مهمة
node-blockquote = اقتباس
node-code-block = كتلة شفرة
node-horizontal-rule = فاصل
node-image = صورة
node-table = جدول
node-table-row = صف جدول
node-table-cell = خلية جدول
node-table-header = رأس جدول
node-block = كتلة

# ─── Login page ─────────────────────────────────────────────────

login-tagline = مستندات بأنياب.
login-error-name-email-required = الاسم والبريد الإلكتروني مطلوبان
login-placeholder-name = الاسم المعروض
login-placeholder-email = البريد الإلكتروني
login-signing-in = جارٍ تسجيل الدخول…
login-dev-button = دخول المطوّر (مخصص)
login-github = تسجيل الدخول بـ GitHub
login-google = تسجيل الدخول بـ Google

# ─── Share dialog ───────────────────────────────────────────────

share-title = مشاركة
share-placeholder-email = أدخل عنوان البريد الإلكتروني
share-button = مشاركة
share-members-heading = الأعضاء الحاليون
share-role-owner = مالك
share-role-edit = يمكنه التحرير
share-role-comment = يمكنه التعليق
share-role-view = يمكنه العرض
share-error-no-folder = ليس للمستند مجلد — لا يمكن المشاركة
share-error-enter-email = أدخل عنوان بريد إلكتروني
share-status-searching = جارٍ البحث…
share-error-search-failed = فشل البحث: { $err }
share-error-no-user = لم يُعثر على مستخدم بالبريد '{ $email }'
share-status-shared-with = تمت المشاركة مع { $name }
share-error-failed = فشلت المشاركة: { $err }

# المشاركة عبر رابط (قسم خاص بالمستند في مربّع حوار المشاركة)
share-link-heading = المشاركة عبر رابط
share-link-mode-off = إيقاف
share-link-mode-view = يمكنه العرض
share-link-mode-edit = يمكنه التحرير
share-link-note = يمكن لأي شخص في مساحة عملك لديه الرابط فتح هذا المستند.
share-link-off = المشاركة عبر رابط متوقفة.
share-link-opt-comments = السماح بالتعليقات
share-link-opt-history = عرض سجل التعديلات
share-link-opt-conversation = عرض المحادثة
share-link-opt-request = السماح بطلبات التحرير
share-link-copy = نسخ الرابط
share-link-copied = تم نسخ الرابط
share-link-saved = تم الحفظ
share-link-error = تعذّر الحفظ: { $err }
# Viewer-facing request-edit-access affordance (#110)
share-link-request-banner = أنت تطّلع على هذا المستند عبر رابط مشترك.
share-link-request-button = طلب صلاحية التحرير
share-link-request-sending = جارٍ الإرسال…
share-link-request-sent = تم إرسال الطلب
share-link-request-retry = تعذّر الإرسال — حاول مرة أخرى

# ─── Document page ──────────────────────────────────────────────

document-loading = جارٍ تحميل المستند…
document-trash-banner = هذا المستند في المهملات — استعده للتعديل.
document-trash-restore = استعادة
document-trash-delete-forever = حذف نهائي
document-share-tooltip = مشاركة
# Document menu: rename + move (#146)
document-rename-prompt = إعادة تسمية المستند
document-move-folder-title = نقل إلى مجلد
document-move-here = نقل إلى هنا
# Duplicate dialog (#146)
duplicate-dialog-title = تكرار المستند
duplicate-name-label = الاسم
duplicate-destination-label = مجلد الوجهة
duplicate-confirm = تكرار
duplicate-share-warning = هذا المجلد مشترك — لدى { $count } أشخاص آخرين حق الوصول وسيرون النسخة.
# Focus/expand toggle (#134)
document-focus-enter = وضع التركيز
document-focus-exit = إنهاء وضع التركيز
document-trash-dialog-title = نقل إلى المهملات
document-trash-dialog-message = سيُنقل هذا المستند إلى المهملات. يمكنك استعادته لاحقًا.
document-trash-dialog-confirm = نقل إلى المهملات
document-purge-dialog-title = حذف نهائي؟
document-purge-dialog-message = هذا يحذف المستند وكل محتوياته بشكل نهائي. لا يمكن التراجع عن هذا الإجراء.
document-purge-dialog-confirm = حذف نهائي
document-restore-folder-title = استعادة إلى مجلد

# ─── Home page ──────────────────────────────────────────────────

home-new-document = + مستند جديد
home-new-spreadsheet = + جدول بيانات جديد
home-new-folder = + مجلد جديد

# ─── MFA (enroll + challenge) ───────────────────────────────────

mfa-verifying = جارٍ التحقق…
mfa-enter-totp = أدخل الرمز المكون من 6 أرقام من تطبيق المصادقة
mfa-enter-recovery = أدخل رمز الاسترداد الخاص بك

mfa-enroll-title = إعداد المصادقة الثنائية
mfa-enroll-subtitle = امسح رمز QR بتطبيق المصادقة، ثم أدخل الرمز المكون من 6 أرقام للتأكيد.
mfa-enroll-success = تم تأكيد التسجيل. جارٍ إعادة التوجيه…
mfa-enroll-error-failed = فشل التسجيل: { $err }
mfa-enroll-error-verify-failed = فشل التحقق: { $err }
mfa-enroll-manual-entry = الإدخال اليدوي
mfa-enroll-recovery-codes-summary = رموز الاسترداد (احفظها الآن!)
mfa-enroll-recovery-warning = يمكن استخدام كل رمز مرة واحدة إذا فقدت الوصول إلى تطبيق المصادقة. لن نعرضها مرة أخرى.
mfa-enroll-code-label = رمز المصادقة
mfa-enroll-confirm = تأكيد

mfa-challenge-title = المصادقة الثنائية
mfa-challenge-subtitle-totp = افتح تطبيق المصادقة وأدخل الرمز المكون من 6 أرقام.
mfa-challenge-subtitle-recovery = أدخل أحد رموز الاسترداد ذات الاستخدام الواحد.
mfa-challenge-verify = تحقق
mfa-challenge-missing-handle = معرّف MFA مفقود، جارٍ إعادة التوجيه إلى تسجيل الدخول…
mfa-challenge-error-invalid-totp = رمز غير صالح — تحقق من تطبيق المصادقة وحاول مجددًا
mfa-challenge-error-invalid-recovery = رمز استرداد غير صالح — يمكن استخدام كل رمز مرة واحدة فقط
mfa-challenge-use-totp = استخدم رمز المصادقة بدلًا من ذلك
mfa-challenge-use-recovery = فقدت تطبيق المصادقة؟ استخدم رمز استرداد

# ─── Admin console (platform-admin pages) ───────────────────────

admin-loading = جارٍ تحميل وحدة تحكم المسؤول…
admin-redirecting = جارٍ إعادة التوجيه…
admin-status-active = نشط
admin-status-disabled = معطّل
admin-status-never = أبدًا
admin-role-admin = مسؤول
admin-role-user = مستخدم
admin-retry = إعادة المحاولة

# Admin sub-nav
admin-nav-users = المستخدمون
admin-nav-metrics = القياسات
admin-nav-audit = السجل
admin-nav-back = العودة إلى التطبيق

# Admin · Users
admin-users-title = مسؤول · المستخدمون
admin-users-search-placeholder = التصفية حسب بداية البريد
admin-users-th-email = البريد الإلكتروني
admin-users-th-name = الاسم
admin-users-th-role = الدور
admin-users-th-state = الحالة
admin-users-th-last-active = آخر نشاط
admin-users-th-actions = الإجراءات
admin-users-enable = تمكين
admin-users-disable = تعطيل
admin-users-promote = ترقية
admin-users-demote = خفض
admin-users-prev = السابق
admin-users-next = التالي
admin-users-error-list-failed = فشل العرض: { $err }
admin-users-error-action-failed = فشل { $action }: { $err }

# Admin · Audit
admin-audit-title = مسؤول · سجل التدقيق
admin-audit-label-target = معرّف المستخدم الهدف
admin-audit-label-actor = معرّف الفاعل
admin-audit-label-kind = النوع
admin-audit-placeholder-kind = مثل disable، loginFailure
admin-audit-label-from = من (ISO)
admin-audit-label-to = إلى (ISO)
admin-audit-search = بحث
admin-audit-error-target-required = معرّف المستخدم الهدف مطلوب
admin-audit-error-load-failed = فشل التحميل: { $err }
admin-audit-th-when = متى
admin-audit-th-source = المصدر
admin-audit-th-kind = النوع
admin-audit-th-actor = الفاعل
admin-audit-th-target = الهدف
admin-audit-th-detail = التفاصيل

# Admin · Metrics
admin-metrics-title = مسؤول · القياسات
admin-metrics-refresh = تحديث
admin-metrics-error-fetch-failed = فشل الجلب: { $err }
admin-metrics-counters = العدّادات
admin-metrics-gauges = المؤشرات
admin-metrics-histograms = المدرّجات
admin-metrics-th-key = المفتاح
admin-metrics-th-value = القيمة
admin-metrics-th-count = العدد
admin-metrics-th-sum = المجموع
admin-metrics-th-min = الأدنى
admin-metrics-th-max = الأعلى

# ─── Workspace SCIM tokens ──────────────────────────────────────

scim-title = رموز SCIM لمساحة العمل
scim-subtitle = أنشئ رمز حامل لموصّل توفير SCIM في موفّر الهوية لديك. يُعرض النص الكامل مرة واحدة عند الإنشاء — انسخه فورًا.
scim-base-url-heading = عنوان URL لقاعدة SCIM
scim-base-url-help = الصق هذا في إعداد موصّل SCIM في موفّر الهوية لديك.
scim-fresh-heading = رمز جديد: { $name }
scim-fresh-warning = انسخ هذا الرمز الآن — لن يُعرض مرة أخرى.
scim-fresh-copy = نسخ
scim-create-heading = إنشاء رمز جديد
scim-create-placeholder = التسمية (مثل موصّل Okta)
scim-create-button = إنشاء
scim-existing-heading = الرموز الموجودة
scim-empty = لا توجد رموز بعد. أنشئ واحدًا أعلاه.
scim-th-name = الاسم
scim-th-token-id = معرّف الرمز
scim-th-created = أُنشئ في
scim-th-last-used = آخر استخدام
scim-th-status = الحالة
scim-status-active = نشط
scim-status-revoked = ملغى
scim-revoke = إلغاء
scim-error-name-required = اسم الرمز مطلوب
scim-error-load-failed = فشل التحميل: { $err }
scim-error-create-failed = فشل الإنشاء: { $err }
scim-error-revoke-failed = فشل الإلغاء: { $err }

# ─── Workspace SAML SSO ─────────────────────────────────────────

saml-title = الدخول الموحّد SAML لمساحة العمل
saml-subtitle-prefix = أعدّ موفّر هوية SAML 2.0 لمساحة العمل هذه. سيتمكن الأعضاء من تسجيل الدخول عبر موفّر الهوية على
saml-subtitle-suffix = بعد الحفظ.
saml-status-saved = تم حفظ إعداد SAML.
saml-status-removed = تمت إزالة إعداد SAML.
saml-status-copied = تم نسخ عنوان بيانات SP إلى الحافظة.
saml-sp-heading = بيانات SP
saml-sp-help = انسخ هذا العنوان في تدفق "إضافة مزود خدمة" في موفّر الهوية. أو اجلب العنوان مرة واحدة والصق XML الناتج في موفّر الهوية.
saml-copy = نسخ
saml-idp-heading = إعداد موفّر الهوية
saml-idp-help = الصق بيانات XML التي يمنحك إياها موفّر الهوية. مطلوب XML الكامل — بما في ذلك العنصر الجذر <EntityDescriptor>.
saml-label-entity-id = معرّف كيان موفّر الهوية
saml-placeholder-entity-id = https://idp.example.com/metadata
saml-label-metadata-xml = بيانات XML لموفّر الهوية
saml-label-email-attr = اسم سمة البريد
saml-label-name-attr = اسم سمة الاسم
saml-save = حفظ
saml-update = تحديث
saml-remove = إزالة
saml-error-entity-id-required = معرّف كيان موفّر الهوية مطلوب
saml-error-metadata-required = بيانات XML لموفّر الهوية مطلوبة
saml-error-load-failed = فشل التحميل: { $err }
saml-error-save-failed = فشل الحفظ: { $err }
saml-error-delete-failed = فشل الحذف: { $err }
saml-meta-first-configured = أول إعداد في
saml-meta-last-updated = ؛ آخر تحديث في

# ─── Spreadsheet view (chrome) ──────────────────────────────────

ss-empty = لا توجد بيانات
ss-format-painter-title = ناسخ التنسيق — انقر لنسخ التنسيق من الخلية النشطة، ثم انقر على الهدف. Shift+نقر للوضع الثابت.
ss-format-painter-status = ناسخ — انقر على خلية للتطبيق، Esc للإلغاء
ss-format-painter-status-sticky = ناسخ (ثابت) — انقر على الخلايا للتطبيق، Esc للإيقاف
ss-sort-tooltip = فرز جدول البيانات…

# Status bar
ss-status-count = العدد: { $value }
ss-status-sum = المجموع: { $value }
ss-status-avg = المتوسط: { $value }
ss-status-min = الأدنى: { $value }
ss-status-max = الأعلى: { $value }

# Sheet tabs
ss-rename-sheet-prompt = إعادة تسمية الورقة:
ss-touch-menu-aria = إجراءات الخلية
ss-ctx-rename = إعادة تسمية
ss-ctx-delete = حذف

# Find / replace bar
ss-find-placeholder = بحث…
ss-replace-placeholder = استبدال…
ss-find-next = التالي
ss-find-replace = استبدال
ss-find-replace-all = استبدال الكل
ss-find-no-results = 0 نتيجة

# Filter dropdown
ss-filter-header = تصفية: { $col }
ss-filter-show-all = إظهار الكل
ss-filter-custom-prompt = تصفية مخصصة (مثل >100، <0، =Done، contains:err، empty، notempty):
ss-filter-custom-button = تصفية مخصصة…
ss-filter-empty-value = (فارغ)

# Sort dialog
ss-sort-title = فرز
ss-sort-range-label = النطاق:
ss-sort-has-headers = الصف الأول يحتوي على رؤوس (تجاوزه أثناء الفرز)
ss-sort-by-label = فرز حسب
ss-sort-then-by-label = ثم حسب
ss-sort-asc = تصاعدي
ss-sort-desc = تنازلي
ss-sort-remove-level-title = إزالة مستوى الفرز هذا
ss-sort-add-level = + إضافة مستوى فرز
ss-sort-cancel = إلغاء
ss-sort-apply = تطبيق
ss-sort-err-parse-range = تعذّر تحليل النطاق. استخدم تنسيق A1، مثل A1:G41.
ss-sort-err-no-keys = أضف مفتاح فرز واحدًا على الأقل.

# Foreign-document consent dialog
ss-foreign-title = يجلب هذا المستند بيانات من مصنفات أخرى
ss-foreign-hint = السماح بالجلب يستخدم صلاحيات قراءة حسابك لتلك المستندات. تستمر الموافقة لهذه الجلسة فقط.
ss-foreign-deny = رفض
ss-foreign-allow = سماح

# ─── Spreadsheet context menu (cell right-click) ────────────────

ss-ctx-menu-insert = إدراج
ss-ctx-menu-delete = حذف
ss-ctx-menu-sort = فرز
ss-ctx-menu-format = تنسيق
ss-ctx-menu-comment = تعليق
ss-ctx-menu-hide = إخفاء / إظهار
ss-ctx-menu-data = بيانات
ss-ctx-menu-cond-fmt = تنسيق شرطي
ss-ctx-menu-validation = التحقق من البيانات
ss-ctx-insert-row-above = إدراج صف أعلاه
ss-ctx-insert-row-below = إدراج صف أدناه
ss-ctx-insert-col-left = إدراج عمود يسارًا
ss-ctx-insert-col-right = إدراج عمود يمينًا
ss-ctx-sort-dialog = فرز…
ss-ctx-delete-row = حذف الصف
ss-ctx-delete-rows = حذف { $count } صفوف
ss-ctx-delete-col = حذف العمود
ss-ctx-delete-cols = حذف { $count } أعمدة
ss-ctx-clear-contents = مسح المحتوى
ss-ctx-sort-a-z = فرز أ → ي
ss-ctx-sort-z-a = فرز ي → أ
ss-ctx-freeze-rows = تجميد الصفوف أعلاه
ss-ctx-unfreeze-rows = إلغاء تجميد الصفوف
ss-ctx-freeze-cols = تجميد الأعمدة على اليسار
ss-ctx-unfreeze-cols = إلغاء تجميد الأعمدة

# Cell validation
ss-ctx-set-checkbox = تعيين كمربع اختيار
ss-ctx-set-dropdown = تعيين كقائمة منسدلة…
ss-ctx-remove-validation = إزالة التحقق
ss-ctx-dropdown-prompt = أدخل خيارات القائمة (مفصولة بفاصلات):

# Conditional formatting
ss-ctx-cond-fmt = التنسيق الشرطي…
ss-ctx-cond-fmt-prompt = التنسيق الشرطي (مثل >100، <0، =Done، contains:error، empty، notempty):
ss-ctx-cond-fmt-color-prompt = لون الخلفية (مثل #ff0000، red، #ffd):
ss-ctx-color-scale = مقياس الألوان…
ss-ctx-color-scale-prompt = مقياس الألوان: منخفض،مرتفع أو منخفض،متوسط،مرتفع (مثل #ff0000،#ffff00،#00ff00):
ss-ctx-data-bar = شريط البيانات…
ss-ctx-data-bar-prompt = لون شريط البيانات:
ss-ctx-icon-set = مجموعة الأيقونات…
ss-ctx-icon-set-prompt = مجموعة الأيقونات: arrows أو traffic

# Charts + pivots
ss-ctx-insert-chart = إدراج مخطط…
ss-ctx-chart-type-prompt = نوع المخطط (bar، line، pie):
ss-ctx-chart-title-prompt = عنوان المخطط:
ss-ctx-chart-unknown-type = نوع مخطط غير معروف. استخدم أحد: bar، line، pie.
ss-ctx-insert-pivot = إدراج جدول محوري…
ss-ctx-pivot-needs-multi = يحتاج الجدول المحوري إلى تحديد متعدد الصفوف والأعمدة. حدد بياناتك (مع صف الرؤوس في الصف 1) وحاول مجددًا.

# CSV import + merge
ss-ctx-import-csv = استيراد CSV…
ss-ctx-merge-cells = دمج الخلايا
ss-ctx-unmerge-cells = إلغاء دمج الخلايا

# Hide / unhide
ss-ctx-hide-row = إخفاء الصف
ss-ctx-unhide-all-rows = إظهار جميع الصفوف
ss-ctx-hide-col = إخفاء العمود
ss-ctx-unhide-all-cols = إظهار جميع الأعمدة

# Cell lock + comments + named ranges
ss-ctx-lock-cell = قفل الخلية
ss-ctx-unlock-cell = إلغاء قفل الخلية
ss-ctx-add-comment = إضافة تعليق…
ss-ctx-edit-comment = تعديل التعليق…
ss-ctx-open-comment = فتح سلسلة التعليقات…
ss-ctx-comment-prompt = تعليق:
ss-ctx-remove-comment = إزالة التعليق
ss-comment-preview-empty = لا توجد رسائل بعد
ss-comment-replies-none = لا توجد ردود
ss-comment-replies-one = رد واحد
ss-comment-replies-many = { $count } ردود
ss-ctx-define-name = تعريف اسم…
ss-ctx-name-prompt = اسم لهذا النطاق:
ss-ctx-remove-name = إزالة الاسم…
ss-ctx-no-named-ranges = لا توجد نطاقات مسماة معرّفة.
ss-ctx-remove-name-prompt = إزالة أي اسم؟ المعرّفة: { $names }

# ─── Pivot table editor ─────────────────────────────────────────

ss-pivot-title = محرّر الجدول المحوري
ss-pivot-foreign-source-label = مصدر خارجي:
ss-pivot-foreign-hint = تحرير المصدر الخارجي غير مدعوم بعد. عدّل إعداد الجدول المحوري عبر سمة JSON أو أزله وأنشئه كجدول محلي.
ss-pivot-layout = التخطيط
ss-pivot-layout-compact = مدمج
ss-pivot-layout-outline = مخطّط
ss-pivot-layout-tabular = جدولي
ss-pivot-totals = المجاميع
ss-pivot-totals-none = لا شيء
ss-pivot-totals-rows = صفوف
ss-pivot-totals-cols = أعمدة
ss-pivot-totals-both = كلاهما
ss-pivot-edit-filter-tooltip = تعديل التصفية
ss-pivot-axis-row = صف
ss-pivot-axis-col = عمود
ss-pivot-labels-header = تسميات { $axis } — { $col }
ss-pivot-close-tooltip = إغلاق
ss-pivot-close-editor-tooltip = إغلاق المحرّر (يظل الجدول المحوري معروضًا)
ss-pivot-filter-all = الكل
ss-pivot-filter-none = لا شيء
ss-pivot-filter-prefix = تصفية { $col }
ss-pivot-source-label = المصدر:
ss-pivot-delete = حذف الجدول المحوري
ss-pivot-section-fields = الحقول
ss-pivot-section-rows = الصفوف
ss-pivot-section-cols = الأعمدة
ss-pivot-section-values = القيم
ss-pivot-section-filters = التصفيات
ss-pivot-search-placeholder = البحث في الحقول…
ss-pivot-bin-width-tooltip = عرض الحاوية

# Date granularity options
ss-pivot-date-year = السنة
ss-pivot-date-quarter = الربع
ss-pivot-date-month = الشهر
ss-pivot-date-day = اليوم
ss-pivot-date-hour = الساعة

# ─── App-level / router ─────────────────────────────────────────

app-not-found = الصفحة غير موجودة

# ─── Accessibility ──────────────────────────────────────────────

a11y-skip-to-content = تخطَّ إلى المحتوى الرئيسي
a11y-toolbar-label = شريط أدوات التنسيق
a11y-toolbar-group-undo = تراجع وإعادة
a11y-toolbar-group-block-type = نوع المقطع
a11y-toolbar-group-inline = تنسيق مضمَّن
a11y-toolbar-group-align = المحاذاة
a11y-toolbar-group-block = تنسيق المقطع
a11y-toolbar-group-insert = إدراج
a11y-file-table-label = المستندات والمجلدات
a11y-breadcrumb-label = مسار التنقّل بين المجلدات

# ─── @-menu (mention picker) ────────────────────────────────────

at-menu-empty = اكتب للبحث عن أشخاص ومستندات…

# ─── File browser (home page table) ─────────────────────────────

file-browser-empty = لا يوجد شيء هنا بعد. أنشئ مستندًا أو مجلدًا.
file-browser-th-title = العنوان
file-browser-th-added = أُضيف في
file-type-folder = مجلد
file-type-document = مستند
file-type-spreadsheet = جدول بيانات
file-type-chat = محادثة

# ─── Folder picker ──────────────────────────────────────────────

folder-picker-not-available =  (غير متاح)

# ─── Formula keyboard ───────────────────────────────────────────

formula-key-backspace = مسح للخلف
formula-key-cancel = إلغاء (Esc)
formula-key-commit = تنفيذ (Enter)
# Mode-switcher tabs (Phase 5 M-P3 piece C).
kbd-mode-standard = أب
kbd-mode-numeric = ١٢٣
kbd-mode-formula = ƒx
kbd-standard-hint = استخدم لوحة مفاتيح جهازك

# ─── Search dialog ──────────────────────────────────────────────

search-placeholder = البحث في المستندات…
search-searching = جارٍ البحث…
search-no-results = لم يُعثر على نتائج
search-dialog-label = ابحث في المستندات أو شغّل الأوامر

# ─── Ask (مساعد RAG) ───────────────────────────────────────────

ask-dialog-title = اسأل المساعد
ask-badge = ذكاء اصطناعي
ask-placeholder = اطرح سؤالًا حول مستنداتك…
ask-empty-hint = يبحث المساعد في مستنداتك ويستشهد بما يجده. اطرح سؤالًا محددًا.
ask-sources-heading = المصادر
ask-error-rate-limit = طلبات كثيرة جدًا. انتظر لحظة ثم أعد المحاولة.
ask-error-disabled = تم تعطيل المساعد لمساحة عملك من قِبل مسؤول.
ask-error-unavailable = المساعد غير متاح مؤقتًا.
sidebar-ask = اسأل

# ─── Relationships ──────────────────────────────────────────────

relationship-heading = ذات صلة
relationship-empty = لا توجد مستندات ذات صلة بعد.
relationship-add-aria = أضف مستندًا ذا صلة
relationship-remove-aria = إزالة هذه العلاقة
relationship-picker-placeholder = ابحث عن مستندات للربط…
relationship-picker-aria = ابحث عن مستندات للربط
relationship-picker-confirm = ربط
relationship-type-aria = نوع العلاقة
relationship-error-self = لا يمكن للمستند الارتباط بنفسه.
relation-type-implements = ينفّذ
relation-type-derived-from = مشتق من
relation-type-depends-on = يعتمد على
relation-type-references = يشير إلى
relation-type-supersedes = يحلّ محل

# ─── Theme selector ─────────────────────────────────────────────

theme-aria-label = السمة
theme-system = اتبع سمة النظام
theme-light = سمة فاتحة
theme-dark = سمة داكنة

# ─── Locale selector ────────────────────────────────────────────

locale-aria-label = اللغة

# ─── Inline selection / comment-bubble ──────────────────────────

selection-toolbar-comment = تعليق على التحديد
comment-highlights-add = إضافة تعليق

# ─── Auth-callback page ─────────────────────────────────────────

auth-complete-status = جارٍ إكمال تسجيل الدخول…

# ─── Sync indicator (Phase 5 M-P3 piece B) ──────────────────────

sync-saved = محفوظ
sync-saving = جارٍ الحفظ…
sync-offline = غير متصل
sync-offline-pending = غير متصل — {$count} في الانتظار
sync-saved-tooltip = تم حفظ تغييراتك.
sync-saving-tooltip = جارٍ إرسال أحدث تغييراتك إلى الخادم…
sync-offline-tooltip = أنت غير متصل. أعد الاتصال للمتابعة في التعاون.
sync-offline-pending-tooltip = أنت غير متصل. {$count} من التغييرات لم تصل إلى الخادم بعد.

# ─── Command palette (Phase 5 M-P4 piece A) ─────────────────────

palette-no-actions = لا توجد أوامر مطابقة.
cmd-go-home = الانتقال إلى الصفحة الرئيسية
cmd-toggle-dark-mode = تبديل الوضع الداكن
cmd-open-trash = فتح سلة المهملات
cmd-sign-out = تسجيل الخروج
cmd-ask = اسأل المساعد
cmd-about-palette = لوحة الأوامر: حول
# Editor-scoped commands (M-P4 piece B).
cmd-bold = غامق
cmd-italic = مائل
cmd-underline = تسطير
cmd-strike = يتوسطه خط
cmd-code = رمز مضمن
cmd-heading-1 = عنوان 1
cmd-heading-2 = عنوان 2
cmd-heading-3 = عنوان 3
cmd-paragraph = فقرة
cmd-bullet-list = قائمة نقطية
cmd-ordered-list = قائمة مرقمة
cmd-task-list = قائمة مهام
cmd-blockquote = اقتباس
cmd-code-block = كتلة رمز
cmd-divider = إدراج فاصل
cmd-insert-table = إدراج جدول
cmd-undo = تراجع
cmd-redo = إعادة

# ─── Home-page drop-to-import (Phase 5 M-P5 piece D) ────────────

home-drop-title = اسحب للاستيراد
home-drop-hint = ماركداون (.md) أو HTML (.html) — حتى 1 ميغابايت
home-import-default-title = مستورد

# ─── Toolbar — embed insert (Phase 5 M-P6 piece B) ──────────────

toolbar-embed = تضمين وسائط (عنوان URL)

# ─── File-browser bulk selection (Phase 5 M-P7 piece C) ─────────

file-browser-th-select = تحديد
bulk-selection-count = {$count} محدد
bulk-selection-cancel = إلغاء
bulk-selection-delete = حذف
bulk-delete-dialog-title = نقل المستندات المحددة إلى سلة المهملات؟
bulk-delete-dialog-message = ستنتقل المستندات المحددة إلى سلة المهملات. يمكنك استعادتها خلال 30 يومًا.
bulk-delete-dialog-confirm = نقل إلى سلة المهملات

# Account menu & settings (account-menu feature)
account-menu-aria = قائمة الحساب
account-menu-profile = الملف الشخصي والحالة
account-menu-settings = الإعدادات
account-menu-shortcuts = اختصارات لوحة المفاتيح
common-redirecting = جارٍ إعادة التوجيه…
settings-title = الإعدادات
settings-aria-tabs = أقسام الإعدادات
settings-coming-soon = هذا القسم قادم قريبًا.
settings-tab-profile = الملف الشخصي
settings-tab-appearance = المظهر
settings-tab-notifications = الإشعارات
settings-tab-accessibility = إمكانية الوصول
settings-tab-help = المساعدة والدعم
settings-a11y-dyslexic-label = خط مناسب لعسر القراءة
settings-a11y-dyslexic-hint = استخدم خطًا أكثر وضوحًا لنص المستند.
settings-a11y-reduce-motion-label = تقليل الحركة
settings-a11y-reduce-motion-hint = يقلل الرسوم المتحركة والانتقالات في جميع أنحاء التطبيق.
settings-appearance-language = اللغة
# BYOK — bring-your-own Anthropic key (#29)
settings-byok-label = مساعد الذكاء الاصطناعي — استخدم مفتاح Anthropic الخاص بك
settings-byok-hint = يُخزَّن في هذا المتصفح فقط ويُرسَل مع طلبات الذكاء الاصطناعي؛ لا يُحفظ على خوادمنا أبدًا. اتركه فارغًا لاستخدام مفتاح مساحة العمل.
settings-byok-active = يتم استخدام مفتاحك
settings-byok-none = يتم استخدام مفتاح مساحة العمل.
settings-byok-save = حفظ المفتاح
settings-byok-clear = إزالة المفتاح
settings-appearance-theme = المظهر
settings-help-shortcuts = اختصارات لوحة المفاتيح
settings-help-shortcut-palette = فتح لوحة الأوامر / البحث
settings-help-shortcut-actions = لوحة الأوامر (الإجراءات)
settings-help-version = الإصدار
settings-notif-email-heading = إشعارات البريد الإلكتروني
settings-notif-all = كل النشاط
settings-notif-mentions = الإشارات فقط
settings-notif-off = إيقاف
settings-notif-hint = يحدد النشاط الذي يرسل لك بريدًا إلكترونيًا. لا تتأثر الإشعارات داخل التطبيق.
settings-profile-name = الاسم المعروض
settings-profile-avatar = رابط الصورة الرمزية
settings-profile-email = البريد الإلكتروني
settings-profile-email-hint = تتم إدارة بريدك الإلكتروني بواسطة مزوّد تسجيل الدخول ولا يمكن تغييره هنا.
settings-save = حفظ التغييرات
settings-saving = جارٍ الحفظ…
settings-saved = تم الحفظ
settings-profile-error = تعذّر حفظ تغييراتك. يرجى المحاولة مرة أخرى.
settings-profile-load-error = تعذّر تحميل ملفك الشخصي. أعد تحميل الصفحة للمحاولة مرة أخرى.
settings-profile-name-required = لا يمكن أن يكون الاسم المعروض فارغًا.
settings-profile-avatar-invalid = يجب أن يبدأ رابط الصورة الرمزية بـ http:// أو https://.
settings-status-heading = الحالة
settings-status-emoji = رمز الحالة التعبيري
settings-status-text = ما هي حالتك؟
settings-status-expiry = المسح بعد
settings-status-expiry-never = عدم المسح
settings-status-expiry-30m = 30 دقيقة
settings-status-expiry-1h = ساعة واحدة
settings-status-expiry-4h = 4 ساعات
settings-status-set = تعيين الحالة
settings-status-clear = مسح الحالة
theme-label-system = النظام
theme-label-light = فاتح
theme-label-dark = داكن

# --- menu bar + editor context menu (i18n backfill) ---
menu-cut = قص
menu-copy = نسخ
menu-paste = لصق
menu-copy-block-link = نسخ رابط الكتلة
menu-copy-original-url = نسخ الرابط الأصلي
menu-convert-to-plain-link = تحويل إلى رابط عادي
menu-bold = عريض
menu-italic = مائل
menu-underline = تسطير
menu-strikethrough = يتوسطه خط
menu-code = رمز
menu-comment = تعليق
menu-alignment = محاذاة
menu-align-left = يسار
menu-align-center = وسط
menu-align-right = يمين
menubar-doc-new = جديد
menubar-doc-share = مشاركة…
menubar-doc-copy-link = نسخ الرابط
menubar-doc-move-folder = نقل إلى مجلد…
menubar-doc-duplicate = تكرار…
menubar-doc-new-template = جديد من قالب…
menubar-doc-export = تصدير
menubar-doc-export-html = HTML
menubar-doc-export-markdown = Markdown (نسخ)
menubar-doc-export-csv = CSV
menubar-doc-export-excel = Excel (.xlsx)
menubar-doc-print = طباعة…
menubar-doc-history = سجل المستند…
menubar-doc-details = تفاصيل المستند…
menubar-doc-rename = إعادة تسمية المستند…
menubar-doc-delete = حذف المستند…
menubar-edit-undo = تراجع
menubar-edit-redo = إعادة
menubar-edit-find = بحث واستبدال
menubar-view-comments = إظهار التعليقات
menubar-view-conversation = إظهار المحادثة
menubar-view-cursors = إظهار المؤشرات
menubar-view-focus = وضع التركيز
menubar-view-line-numbers = إظهار أرقام الأسطر
menubar-view-page-breaks = إظهار فواصل الصفحات
menubar-view-outline = إظهار المخطط
menubar-format-subscript = منخفض
menubar-format-superscript = مرتفع
menubar-format-paragraph-style = نمط الفقرة
menubar-format-list = قائمة
menubar-format-clear = مسح التنسيق
menubar-format-lock = قفل التعديلات
editorctx-paragraph-style = نمط الفقرة
editorctx-insert-link = إدراج رابط…

# Editor width toggle (S/M/L)
editor-width-group = عرض المحرر
editor-width-narrow = عرض ضيق
editor-width-medium = عرض متوسط
editor-width-wide = عرض واسع

# ─── Block deep links (#b= fragment consumption) ─────────────────

doc-block-link-missing = القسم المرتبط لم يعد موجودًا.

# ─── Mentions spec §5 (Task 5) — per-viewer degradation overlay ──

doc-mention-missing = مستند مفقود
