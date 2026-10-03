// Result contracts shared by the CLI and its regression tests. Scenario code
// may catch a browser error to collect screenshots; that must still fail CI.
export function evaluateScenarioResult(scenario, collector, ok = true, errMsg = null) {
  const caughtError = ["stepError", "editorError", "assertError", "shareApiError"]
    .find((key) => collector[key]);
  if (ok && caughtError) {
    ok = false;
    errMsg = `${scenario}: ${collector[caughtError]}`;
  }

  if (ok && scenario === "collab-sync") {
    const required = ["syncObservedInB", "remoteCursorObservedInB"];
    const missing = required.filter((key) => !collector.scenario?.[key]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `${scenario} observations failed: ${missing.join(", ")}`;
    }
  }

  // trash-flow reports a `steps` map of what succeeded; if any step is
  // missing/false we flip ok=false so CI treats it as a failure even when
  // no exception was thrown.
  if (ok && scenario === "trash-flow") {
    const s = collector.scenario?.steps || {};
    const required = [
      "docLoaded",
      "deletedAndHomeNav",
      "trashRowVisible",
      "trashedDocListedWithActions",
      "trashBannerShown",
      "editorReadonly",
      "restoredAndHomeNav",
      "docBackInHome",
      "purgedFromTrash",
      "purgedApi404",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `trash-flow steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "mfa-flow") {
    const s = collector.scenario?.steps || {};
    const required = [
      "initialLogin",
      "enrollPageRenderedSecret",
      "recoveryCodesDisplayed",
      "verifyFinalizedEnrollment",
      "loggedOutAfterEnroll",
      "reloginReturnedMfaPending202",
      "totpChallengeMintedSession",
      "postChallengeMeWorks",
      "recoveryCodeMintedSession",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg =
        `mfa-flow steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "admin-console") {
    const s = collector.scenario?.steps || {};
    const required = [
      "adminUsersPageMounted",
      "peerRowVisibleAfterSearch",
      "peerDisabled",
      "peerReEnabled",
      "auditRowsVisible",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `admin-console steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "comment-live-sync") {
    const s = collector.scenario?.steps || {};
    const required = [
      "tabAPopupOpen",
      "tabBPopupOpen",
      "popupBodyOverflowed",
      "tabBScrolledOnOpen",
      "replyPosted",
      "tabBSawReply",
      "tabASawReply",
      "tabBScrolledAfterReply",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `comment-live-sync steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "spreadsheet-features") {
    const s = collector.scenario?.steps || {};
    const required = [
      "gridMounted",
      "toolbarPresent",
      "formatPainterPresent",
      "sheetTabsPresent",
      "formulaBarPresent",
      "cellsPopulated",
      "sortDialogOpened",
      "sortDialogClosedAfterApply",
      "sortReorderedRows",
      "contextMenuOpens",
      "frozenRowClassApplied",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `spreadsheet-features steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "favorites") {
    const s = collector.scenario?.steps || {};
    const required = [
      "startsUnstarred",
      "starButtonGoesActive",
      "appearsInSidebar",
      "unstarButtonGoesInactive",
      "leavesSidebar",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `favorites steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "find-replace") {
    const s = collector.scenario?.steps || {};
    const required = [
      "barOpensFromMenu",
      "barOpensFromCtrlF",
      "countShowsThreeMatches",
      "nextAdvancesMatch",
      "replaceAllRewritesDoc",
      "noMatchesAfterReplace",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `find-replace steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "line-numbers") {
    const s = collector.scenario?.steps || {};
    const required = [
      "editorTyped",
      "numbersAppear",
      "perVisualLine",
      "uniformFont",
      "noFullWidthPageRule",
      "togglesOff",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `line-numbers steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "document-details") {
    const s = collector.scenario?.steps || {};
    const required = [
      "panelOpens",
      "hasAllRows",
      "wordCountCorrect",
      "charCountCorrect",
      "panelCloses",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `document-details steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "subscript") {
    const s = collector.scenario?.steps || {};
    const required = ["subscriptRenders", "switchesToSuperscript", "noPageErrors"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `subscript steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "expand") {
    const s = collector.scenario?.steps || {};
    const required = [
      "startsCollapsed",
      "entersExpanded",
      "headerHidden",
      "fabShown",
      "collapses",
      "headerBack",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `expand steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "doc-actions") {
    const s = collector.scenario?.steps || {};
    const required = [
      "renameUpdatesTitle",
      "duplicateDialogPrefillsName",
      "duplicateNavigatedToNewDoc",
      "duplicateUsesEnteredName",
      "duplicateCopiedContent",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `doc-actions steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "menu-switch") {
    const s = collector.scenario?.steps || {};
    const required = [
      "documentOpened",
      "switchedToViewInOneClick",
      "switchedToFormatInOneClick",
      "sameNameCloses",
      "noPageErrors",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `menu-switch steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "focus-mode") {
    const s = collector.scenario?.steps || {};
    // #134: the toggle must enter + exit via both the button and the
    // shortcut, the chrome must hide/show, and nothing may panic.
    const required = [
      "startsUnfocused",
      "menuVisibleInitially",
      "buttonEntersFocus",
      "menuHiddenInFocus",
      "toggleStillPresentInFocus",
      "buttonExitsFocus",
      "menuVisibleAfterExit",
      "shortcutEntersFocus",
      "shortcutExitsFocus",
      "noPageErrors",
      "noPanicConsole",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `focus-mode steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "spreadsheet-lifecycle") {
    const s = collector.scenario?.steps || {};
    // #76: every loop must complete, the post-unmount copy/paste must land
    // its value (proves the surviving doc's engine is still live), and the
    // run must produce no panic — the use-after-free this guards against
    // surfaces as a Rust panic on a reclaimed engine.
    const required = [
      "bothDocsCreated",
      "loopsCompleted",
      "postUnmountPasteWorks",
      "noPageErrors",
      "noPanicConsole",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `spreadsheet-lifecycle steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "history-pane") {
    const s = collector.scenario?.steps || {};
    // Must actually open a diff modal (the panic only fires on modal
    // teardown), then close it with no pageerror. modalOpen guards
    // against a false green if version seeding ever stops producing a
    // version to browse.
    const required = ["editorMounted", "paneOpened", "modalOpen", "noPanic"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      const errs = (collector.scenario?.errors || [])
        .map((e) => e.message)
        .join("; ");
      errMsg = `history-pane steps failed: ${missing.join(", ")}` +
        (errs ? ` [pageerrors: ${errs}]` : "") +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "delete-document") {
    const s = collector.scenario?.steps || {};
    const required = ["editorMounted", "confirmShown", "noPanicOnDelete"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      const errs = (collector.scenario?.errors || [])
        .map((e) => e.message)
        .join("; ");
      errMsg = `delete-document steps failed: ${missing.join(", ")}` +
        (errs ? ` [pageerrors: ${errs}]` : "") +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && (scenario === "share-dialog" || scenario === "comment-popup")) {
    const s = collector.scenario?.steps || {};
    const shownKey = scenario === "share-dialog" ? "dialogShown" : "popupShown";
    const required = ["editorMounted", shownKey, "noPanic"];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      const errs = (collector.scenario?.errors || []).map((e) => e.message).join("; ");
      errMsg = `${scenario} steps failed: ${missing.join(", ")}` +
        (errs ? ` [pageerrors: ${errs}]` : "") +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "pivot-editor") {
    const s = collector.scenario?.steps || {};
    const required = [
      "gridMounted",
      "cellsPopulated",
      "contextMenuOpens",
      "editorOpened",
      "fieldListPopulated",
      "chipsAppearedInZone",
      "summarizeFnPickerPresent",
      "editorClosedAfterDelete",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `pivot-editor steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  if (ok && scenario === "spreadsheet-paste") {
    const s = collector.scenario?.steps || {};
    const required = [
      "gridMounted",
      "cellsPopulated",
      "pasteExecuted",
      "pastedFormulaTranslated",
      "pastedValueIs12",
      "noPermissionDialog",
    ];
    const missing = required.filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `spreadsheet-paste steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  // ─── May 2026 UI batch — six required-steps gates ───────────────
  const requiredSteps = {
    "calendar-block": [
      "editorReady", "calendarBlockRendered", "monthGridPresent", "sevenWeekdayHeaders",
      "addModalOpened", "eventVisibleAfterSave", "eventPersistsAfterReload",
      "editModalHeaderCorrect", "editModalTitlePrefilled", "deleteButtonInEditMode", "noPageErrors",
    ],
    "kanban-block": [
      "editorReady", "kanbanBlockRendered", "threeDefaultColumns", "addModalOpened",
      "cardVisibleAfterSave", "cardLandedInFirstColumn", "cardPersistsAfterReload",
      "editModalTitlePrefilled", "deleteButtonInEditMode", "noPageErrors",
    ],
    "kanban-drag": [
      "editorReady", "kanbanInserted", "twoCardsInFirstColumn", "cardLeftSourceColumn",
      "cardArrivedInDestColumn", "movePersistsAfterReload", "noPageErrors",
    ],
    "kanban-column-reorder": [
      "initialOrderIsDefault", "columnReorderedInDom", "columnOrderPersistsAfterReload", "noPageErrors",
    ],
    "kanban-wip-limit": [
      "wipLimitVisibleInPill", "firstCardAddedSuccessfully", "secondCardBlockedByWipLimit",
      "wipLimitPersistsAfterReload", "noPageErrors",
    ],
    "kanban-card-metadata": [
      "dueDateRenderedOnCard", "labelChipsRendered", "assigneeInitialsRendered",
      "dueDatePreFilledInEdit", "labelsPreFilledInEdit", "assigneePreFilledInEdit", "noPageErrors",
    ],
    "type-past-atom": [
      "editorReady", "calendarInsertedWithAtomSize", "clickedTrailingParagraph", "noInsertTextFailed",
      "noHistoryApiThrottle", "caretNotInsideCalendar", "typedTextInTrailingParagraph",
      "typedTextPersistsAfterReload", "noPageErrors",
    ],
    "settings-appearance": [
      "stylesheetApplied", "threeThemeButtons", "panelStretches",
      "themeButtonsInsidePanel", "noPageErrors",
    ],
    "spreadsheet-keyboard": [
      "gridMounted", "valueTyped", "undoCleared", "redoRestored",
    ],
    "spreadsheet-headers": [
      "gridMounted", "cellsPopulated", "colHeaderClickSelectsColumn",
      "statusBarShowsCount", "statusBarShowsSum",
    ],
    "spreadsheet-toolbar": [
      "gridMounted", "toolbarPresent", "formatDropdownIsFormat",
      "currencyAppliedToCell", "boldButtonAppliedFontWeight",
    ],
    "spreadsheet-freeze": [
      "gridMounted", "contextMenuOpens",
      "frozenAboveExcludesClickedRow", "clickedRowNotFrozen",
    ],
    "spreadsheet-sheet-tabs": [
      "gridMounted", "secondTabAdded", "tabContextMenuOpened",
      "contextMenuFitsInViewport", "deleteRemovesTab",
    ],
    "spreadsheet-remote-cursor": [
      "tabAGridMounted", "tabBGridMounted", "tabANavigated",
      "tabBSeesRemoteCell",
    ],
    "mobile-spreadsheet-keyboards": [
      "gridMounted", "touchPrimaryActive", "cellEditing",
      "formulaKeyboardMounted", "autocompleteVisible",
      "autocompleteAboveKeyboard", "firstMatchIsSum",
    ],
    "command-palette-actions": [
      "editorReady", "textTyped", "textSelected",
      "paletteOpened", "paletteInActionMode",
      "boldVisible", "enterDispatched", "dialogClosed",
      "boldApplied",
    ],
    "embed-youtube": [
      "editorReady", "embedButtonVisible", "iframeInserted",
      "srcRewrittenToEmbed", "sandboxAllowsScripts",
      "referrerPolicyCorrect", "loadingLazy",
      "wrapperContenteditableFalse",
    ],
    "bulk-delete": [
      "fileBrowserMounted", "threeBoxesChecked",
      "selectionBarVisible", "countShows3", "confirmDialogOpened",
      "selectionBarDismissed", "docsRemovedFromHome",
    ],
    "a11y-audit": [
      "homeMounted", "homeNoSeriousOrCritical",
      "editorMounted", "editorNoSeriousOrCritical",
      "paletteMounted", "paletteNoSeriousOrCritical",
    ],
    "ask-flow": [
      "homeMounted", "paletteOpened", "askDialogOpened",
      "questionSubmitted", "askEndpointAvailable",
      "answerStreamed", "sourcesAppeared",
      "firstCitationMatchesSeed",
    ],
    "code-block-enter": [
      "editorReady", "aCodeBlockCreated", "aChipShowsPython",
      "bKeywordSpan", "cSinglePre", "cTextContent", "cAutoIndent",
      "cSelectionInPre", "dTextContent", "dSinglePre",
      "eParagraphAfterPre", "eSelectionInParagraph",
      "eTextContentUnchanged", "noPageErrors", "noConsoleErrors",
    ],
    "block-links": [
      "blocksHaveIds", "targetBlockId", "menuItemVisible",
      "clipboardUrlCorrect", "validFragmentFlash", "validFragmentNoToast",
      "foreignHashInertAppearance", "foreignHashInertEmptyB",
      "foreignHashInertMalformedB", "missingBlockToastShown",
      "missingBlockToastText", "missingBlockToastAutodismiss",
      "noConsoleErrors",
    ],
    "doc-mentions": [
      "t1BlockId", "pasteDocUrlConverts", "docChipGlyphTitle",
      "singleUndoRestoresUrl", "pasteAnchorUrlConverts",
      "anchorChipGlyphSnippet", "pasteDanglingConverts",
      "noAccessStaysPlain", "urlInSentenceUntouched",
      "danglingClassApplied", "danglingDocGlyph", "ctxEntriesVisible",
      "ctxCopyOriginalUrl", "ctxAbsentOnPlainText", "chipClickNavigates",
      "t2Trashed", "missingClassApplied", "missingLabel",
      "missingClickInert", "convertToPlainLink", "noConsoleErrors",
    ],
    "deck-basics": [
      "newButtonVisible", "newMenuItemVisible", "navigatedToDoc",
      "canvasRendered", "initialThumbCount", "presetPickerVisible",
      "slideAdded", "bodyFramePlaceholderVisible", "inlineEditorMounted",
      "editorClosedAfterEscape", "canvasRenderedAfterReload",
      "thumbCountPersisted", "typedTextPersisted", "thirdSlideAdded",
      "duplicateIncreasedCount", "duplicateAddedNewBlockId",
      "deleteDecreasedCount", "deleteRemovedTargetBlockId",
      "noConsoleErrors",
    ],
    "deck-blocks": [
      "canvasRendered", "slideAdded", "editorMounted", "slashMenuOpen",
      "mermaidInserted", "mermaidOnCanvas", "imageUploaded",
      "imageOnCanvas", "persistedBeforeReload", "reloadMermaidPersisted",
      "reloadImagePersisted", "noConsoleErrors",
    ],
    "deck-present": [
      "presentRouteReached", "stageRendersSlide", "slideTextVisible",
      "arrowAdvances", "arrowGoesBack", "escapeReturnsToEditor",
      "presenterViewPanels", "timerAdvances", "pdfExportDownloads",
      "noConsoleErrors",
    ],
  };
  if (ok && Object.prototype.hasOwnProperty.call(requiredSteps, scenario)) {
    const s = collector.scenario?.steps || {};
    const missing = requiredSteps[scenario].filter((k) => !s[k]);
    if (missing.length > 0) {
      ok = false;
      errMsg = `${scenario} steps failed: ${missing.join(", ")}` +
        (collector.stepError ? ` (${collector.stepError})` : "");
    }
  }

  // Any uncaught page error (a WASM panic surfaces as one) fails the
  // run, not just the four scenarios that opted in via `panicRe`. The
  // "closure invoked recursively or after being dropped" class was
  // captured-and-ignored in the other fifty. Allowlist by scenario
  // name only when a scenario knowingly provokes an error.
  const PAGEERROR_ALLOWLIST = new Set([]);
  // Environmental, not a bug: a navigation that starts while the WASM
  // module is still streaming aborts that fetch, and the browser reports
  // it as a pageerror. Nothing in the app ran.
  const ENVIRONMENTAL_PAGEERROR = /WebAssembly compilation aborted: Network error/;
  if (!PAGEERROR_ALLOWLIST.has(scenario)) {
    for (const tag of Object.keys(collector)) {
      const errs = ((collector[tag] && Array.isArray(collector[tag].errors)) ? collector[tag].errors : [])
        .filter((e) => !ENVIRONMENTAL_PAGEERROR.test(String(e && e.message)));
      if (errs.length > 0) {
        console.error(`[doctor] ${errs.length} page error(s) on ${tag}; failing run`);
        for (const e of errs) console.error(`  - ${e.message}`);
        if (ok) errMsg = `${scenario}: ${errs.length} page error(s) on ${tag}: ${errs[0].message}`;
        ok = false;
      }
    }
  }

  return { ok, error: errMsg };
}
