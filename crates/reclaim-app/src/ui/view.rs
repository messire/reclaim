//! Slint markup for every MVP screen and the tray. Static labels use `@tr(...)` so a
//! translation catalogue can replace them without code changes (PRODUCT §7).
//!
//! Visual style: dark, flat, monospace (Consolas, shipped with Windows; no embedded
//! font). Everything here is cheap for the software renderer on purpose: flat fills,
//! 1 px lines, no blur, no shadows, no glow and no running animations. The background
//! grid is a set of static 1 px rectangles, so it is only repainted inside regions that
//! change for another reason.

slint::slint! {
    #[edition = "1.19"]
    import {
        CheckBox, LineEdit, TextEdit, ListView, Palette, AboutSlint, ComboBox,
    } from "std-widgets.slint";

    export struct ProcRow {
        selected: bool,
        name: string,
        pid: string,
        working-set: string,
        private-bytes: string,
        status: string,
        eligible: bool,
    }

    export struct DriveItem {
        root: string,
        title: string,
        detail: string,
        used: int,
    }

    export struct TreeRow {
        id: int,
        depth: int,
        name: string,
        size: string,
        share: int,
        parent-pct: string,
        files: string,
        modified: string,
        expandable: bool,
        expanded: bool,
        selected: bool,
        flags: string,
        more: bool,
        color: color,
    }

    export struct Tile {
        x: float,
        y: float,
        w: float,
        h: float,
        color: color,
        leaf: bool,
    }

    export struct ListRow {
        id: int,
        name: string,
        detail: string,
        size: string,
        extra: string,
        color: color,
    }

    export struct CatRow {
        id: string,
        name: string,
        safety: string,
        level: int,
        size: string,
        files: string,
        selected: bool,
        enabled: bool,
        focused: bool,
    }

    export struct CandRow {
        path: string,
        size: string,
        modified: string,
        selected: bool,
    }

    export struct DupRow {
        group: int,
        file: int,
        text: string,
        size: string,
        marked: bool,
        header: bool,
    }

    global Theme {
        out property <color> bg: #0B1013;
        out property <color> panel: #0a0e11;
        out property <color> grid: #10181d;
        out property <color> line: #1b2830;
        out property <color> text: #c3cdd1;
        out property <color> muted: #8a9da5;
        out property <color> dim: #62777f;
        out property <color> accent: #3fe3f2;
        out property <color> accent-dim: #1fa3b3;
        out property <color> track: #14202a;
        out property <color> amber: #c9a86a;
        out property <color> warn: #e0a458;
        out property <color> error: #f0766e;
        out property <color> hover: #ffffff0a;
        // Memory-in-use bar: teal while there is room, amber, then red near full.
        out property <brush> usage: @linear-gradient(90deg, #1fa3b3 0%, #3fe3f2 40%, #e0a458 75%, #f0766e 100%);
        out property <length> cell: 28px;
        out property <length> spacing: 1.5px;
        // Process table column widths (name and status stretch).
        out property <length> col-mark: 16px;
        out property <length> col-pid: 64px;
        out property <length> col-bytes: 116px;
    }

    // Faint background grid. Static: the renderer only repaints it under dirty regions.
    component GridBackground inherits Rectangle {
        background: Theme.bg;
        for i in ceil(root.width / Theme.cell) : Rectangle {
            x: i * Theme.cell;
            y: 0;
            width: 1px;
            height: root.height;
            background: Theme.grid;
        }
        for i in ceil(root.height / Theme.cell) : Rectangle {
            x: 0;
            y: i * Theme.cell;
            width: root.width;
            height: 1px;
            background: Theme.grid;
        }
    }

    component Panel inherits Rectangle {
        background: Theme.panel;
        border-width: 1px;
        border-color: Theme.line;
        border-radius: 6px;
    }

    component Rule inherits Rectangle {
        height: 1px;
        background: Theme.line;
    }

    // Uppercase caption with wide tracking.
    component Caption inherits Text {
        in property <string> label;
        text: root.label.to-uppercase();
        color: Theme.muted;
        letter-spacing: Theme.spacing;
        vertical-alignment: center;
    }

    // Bracketed amber value, as in "[ 8.1 GiB of 16 GiB ]".
    component Bracket inherits Text {
        in property <string> value;
        text: "[ " + root.value + " ]";
        color: Theme.amber;
        letter-spacing: Theme.spacing;
        horizontal-alignment: right;
        vertical-alignment: center;
    }

    // Flat progress bar; `permille` is 0..1000. The fill brush spans the whole track and
    // is clipped to the filled part, so a gradient shows the colour of the current level.
    component Bar inherits Rectangle {
        in property <int> permille;
        in property <brush> fill: Theme.accent;
        height: 14px;
        background: Theme.track;
        Rectangle {
            x: 0;
            width: parent.width * clamp(root.permille, 0, 1000) / 1000;
            height: parent.height;
            clip: true;
            Rectangle {
                x: 0;
                width: root.width;
                height: parent.height;
                background: root.fill;
            }
        }
    }

    component Value inherits HorizontalLayout {
        in property <string> label;
        in property <string> value;
        in property <color> value-color: Theme.text;
        spacing: 12px;
        Caption { label: root.label; min-width: 220px; horizontal-stretch: 0; font-size: 12px; color: Theme.dim; }
        Text { text: root.value; color: root.value-color; vertical-alignment: center; horizontal-stretch: 1; }
    }

    component FlatButton {
        in property <string> text;
        in property <bool> primary;
        in property <bool> enabled: true;
        callback clicked();
        min-height: 32px;
        min-width: label.preferred-width + 28px;
        preferred-width: label.preferred-width + 28px;
        horizontal-stretch: 0;
        vertical-stretch: 0;

        focus := FocusScope {
            enabled: root.enabled;
            key-pressed(e) => {
                if (e.text == Key.Return || e.text == Key.Space) {
                    root.clicked();
                    return accept;
                }
                return reject;
            }
        }
        Rectangle {
            border-width: 1px;
            border-radius: 3px;
            border-color: !root.enabled ? Theme.line
                : root.primary ? Theme.accent
                : focus.has-focus || touch.has-hover ? Theme.muted
                : Theme.line;
            background: root.enabled && touch.has-hover ? Theme.hover : transparent;
        }
        label := Text {
            text: root.text.to-uppercase();
            color: !root.enabled ? Theme.dim : root.primary ? Theme.accent : Theme.text;
            letter-spacing: 1px;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
        touch := TouchArea {
            enabled: root.enabled;
            mouse-cursor: pointer;
            clicked => {
                focus.focus();
                root.clicked();
            }
        }
    }

    component TabItem {
        in property <string> text;
        in property <bool> active;
        callback clicked();
        min-height: 36px;
        max-height: 36px;
        preferred-width: label.preferred-width + 24px;
        horizontal-stretch: 0;
        vertical-stretch: 0;
        label := Text {
            text: root.text.to-uppercase();
            font-size: 12px;
            letter-spacing: Theme.spacing;
            color: root.active ? Theme.accent : touch.has-hover ? Theme.text : Theme.muted;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
        if root.active : Rectangle {
            x: 8px;
            y: root.height - 2px;
            width: root.width - 16px;
            height: 2px;
            background: Theme.accent;
        }
        touch := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
        }
    }

    // Square icon button on the right of the tab row (Settings, About). The icon is a
    // stroked 24×24 path, so it needs no font glyph and no image asset.
    component IconTab {
        in property <string> commands;
        in property <bool> active;
        in property <string> tip;
        callback clicked();
        width: 36px;
        height: 36px;
        horizontal-stretch: 0;
        vertical-stretch: 0;
        accessible-role: button;
        accessible-label: root.tip;
        Path {
            x: (parent.width - 18px) / 2;
            y: (parent.height - 18px) / 2;
            width: 18px;
            height: 18px;
            viewbox-width: 24;
            viewbox-height: 24;
            commands: root.commands;
            fill: transparent;
            stroke: root.active ? Theme.accent : touch.has-hover ? Theme.text : Theme.muted;
            stroke-width: 1.5px;
        }
        if root.active : Rectangle {
            x: 6px;
            y: root.height - 2px;
            width: root.width - 12px;
            height: 2px;
            background: Theme.accent;
        }
        touch := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
        }
    }

    // Sortable header cell of the process table.
    component HeadCell {
        in property <string> text;
        in property <int> column;
        in property <int> sorted;
        in property <bool> ascending;
        callback clicked();
        min-height: 36px;
        max-height: 36px;
        vertical-stretch: 0;
        Text {
            x: 0;
            width: parent.width;
            text: root.text.to-uppercase()
                + (root.sorted == root.column ? (root.ascending ? " ↑" : " ↓") : "");
            font-size: 12px;
            letter-spacing: Theme.spacing;
            color: root.sorted == root.column ? Theme.muted : Theme.dim;
            vertical-alignment: center;
            overflow: elide;
        }
        TouchArea {
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
        }
    }

    // Small text link (inline action).
    component Link inherits Text {
        in property <bool> enabled: true;
        callback clicked();
        font-size: 12px;
        letter-spacing: 1px;
        color: !root.enabled ? Theme.dim : link-touch.has-hover ? Theme.text : Theme.accent;
        vertical-alignment: center;
        link-touch := TouchArea {
            enabled: root.enabled;
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
        }
    }

    // Directory tree with size bars; virtualised by ListView. `scroll-serial` changes
    // whenever the selection moves, so the selected row is scrolled into view.
    component TreeView inherits Panel {
        in property <[TreeRow]> rows;
        in property <int> scroll-to: -1;
        in property <int> scroll-serial;
        callback select(int);
        callback toggle(int);
        callback more(int);
        callback zoom(int);
        private property <length> row-h: 24px;

        changed scroll-serial => {
            if (root.scroll-to >= 0) {
                if (root.scroll-to * root.row-h < -list.content-y
                    || (root.scroll-to + 1) * root.row-h > -list.content-y + list.visible-height) {
                    list.content-y = min(0px, -(root.scroll-to * root.row-h - list.visible-height / 2));
                }
            }
        }

        VerticalLayout {
            padding-left: 8px;
            padding-right: 8px;
            padding-bottom: 4px;
            HorizontalLayout {
                spacing: 8px;
                min-height: 30px;
                max-height: 30px;
                Caption { label: @tr("Name"); font-size: 11px; horizontal-stretch: 1; }
                Caption { label: @tr("Size"); font-size: 11px; width: 160px; }
                Caption { label: @tr("Parent"); font-size: 11px; width: 52px; }
                Caption { label: @tr("Files"); font-size: 11px; width: 64px; }
            }
            Rule {}
            list := ListView {
                vertical-stretch: 1;
                for r in root.rows : Rectangle {
                    height: root.row-h;
                    background: r.selected ? #3fe3f224 : row-touch.has-hover ? Theme.hover : transparent;
                    row-touch := TouchArea {
                        mouse-cursor: pointer;
                        clicked => {
                            if (r.more) {
                                root.more(r.id);
                            } else {
                                root.select(r.id);
                            }
                        }
                        double-clicked => {
                            if (!r.more) {
                                root.zoom(r.id);
                            }
                        }
                    }
                    HorizontalLayout {
                        padding-left: r.depth * 14px;
                        spacing: 6px;
                        Text {
                            width: 14px;
                            text: r.expandable ? (r.expanded ? "▾" : "▸") : "";
                            color: Theme.muted;
                            vertical-alignment: center;
                            horizontal-alignment: center;
                            TouchArea {
                                mouse-cursor: pointer;
                                clicked => { root.toggle(r.id); }
                            }
                        }
                        Rectangle {
                            width: 8px;
                            Rectangle {
                                width: 8px;
                                height: 8px;
                                background: r.color;
                            }
                        }
                        Text {
                            horizontal-stretch: 1;
                            preferred-width: 0;
                            text: r.flags == "" ? r.name : r.name + "  · " + r.flags;
                            color: r.more ? Theme.dim : r.selected ? Theme.accent : Theme.text;
                            vertical-alignment: center;
                            overflow: elide;
                        }
                        Rectangle {
                            width: 70px;
                            Rectangle {
                                x: 0;
                                width: parent.width;
                                height: 6px;
                                background: Theme.track;
                                Rectangle {
                                    x: 0;
                                    width: parent.width * clamp(r.share, 0, 1000) / 1000;
                                    background: Theme.accent-dim;
                                }
                            }
                        }
                        Text {
                            width: 84px;
                            text: r.size;
                            color: Theme.amber;
                            horizontal-alignment: right;
                            vertical-alignment: center;
                        }
                        Text {
                            width: 52px;
                            text: r.parent-pct;
                            color: Theme.muted;
                            horizontal-alignment: right;
                            vertical-alignment: center;
                        }
                        Text {
                            width: 64px;
                            text: r.files;
                            color: Theme.dim;
                            horizontal-alignment: right;
                            vertical-alignment: center;
                        }
                    }
                }
            }
        }
    }

    // Treemap or icicle. Tiles come in map coordinates (logical pixels of this area);
    // one TouchArea does hover, click and double-click for all of them.
    component MapView inherits Rectangle {
        in property <[Tile]> tiles;
        in property <bool> has-sel;
        in property <float> sel-x;
        in property <float> sel-y;
        in property <float> sel-w;
        in property <float> sel-h;
        callback resized(float, float);
        pure callback hover(float, float) -> string;
        callback click(float, float);
        callback zoom(float, float);
        private property <string> tip;

        background: Theme.track;
        clip: true;
        init => { root.resized(self.width / 1px, self.height / 1px); }
        changed width => { root.resized(self.width / 1px, self.height / 1px); }
        changed height => { root.resized(self.width / 1px, self.height / 1px); }

        for t in root.tiles : Rectangle {
            x: t.x * 1px;
            y: t.y * 1px;
            width: t.w * 1px;
            height: t.h * 1px;
            background: t.leaf ? t.color : transparent;
            border-width: t.leaf ? (t.w > 4 && t.h > 4 ? 1px : 0px) : 1px;
            border-color: t.leaf ? #0B101399 : #0B1013;
        }
        if root.has-sel : Rectangle {
            x: root.sel-x * 1px;
            y: root.sel-y * 1px;
            width: max(2px, root.sel-w * 1px);
            height: max(2px, root.sel-h * 1px);
            border-width: 2px;
            border-color: #ffffff;
        }
        touch := TouchArea {
            changed mouse-x => { root.tip = self.has-hover ? root.hover(self.mouse-x / 1px, self.mouse-y / 1px) : ""; }
            changed mouse-y => { root.tip = self.has-hover ? root.hover(self.mouse-x / 1px, self.mouse-y / 1px) : ""; }
            changed has-hover => {
                if (!self.has-hover) {
                    root.tip = "";
                }
            }
            clicked => { root.click(self.mouse-x / 1px, self.mouse-y / 1px); }
            double-clicked => { root.zoom(self.mouse-x / 1px, self.mouse-y / 1px); }
        }
        if root.tip != "" : Rectangle {
            x: min(touch.mouse-x + 14px, max(0px, root.width - self.width - 4px));
            y: touch.mouse-y + 18px + self.height > root.height
                ? max(0px, touch.mouse-y - self.height - 8px)
                : touch.mouse-y + 18px;
            width: min(tip-text.preferred-width + 16px, 420px);
            height: tip-text.preferred-height + 10px;
            background: #0B1013ee;
            border-width: 1px;
            border-color: Theme.accent-dim;
            border-radius: 3px;
            tip-text := Text {
                x: 8px;
                y: 5px;
                width: parent.width - 16px;
                text: root.tip;
                font-size: 12px;
                color: Theme.text;
                wrap: word-wrap;
            }
        }
    }

    // Bottom result list of the analyzer.
    component ResultList inherits Panel {
        in property <[ListRow]> rows;
        in property <int> sorted: -1;
        in property <bool> ascending;
        in property <bool> sortable: true;
        callback sort(int, bool);
        callback select(int);

        function header(column: int) {
            if (!root.sortable) {
                return;
            }
            if (column == root.sorted) {
                root.sort(column, !root.ascending);
            } else {
                root.sort(column, column == 1 || column == 2);
            }
        }

        VerticalLayout {
            padding-left: 8px;
            padding-right: 8px;
            HorizontalLayout {
                spacing: 8px;
                HeadCell { text: "·"; column: 2; width: 16px; sorted: root.sorted; ascending: root.ascending; clicked => { root.header(2); } }
                HeadCell { text: @tr("Name"); column: 1; width: 200px; sorted: root.sorted; ascending: root.ascending; clicked => { root.header(1); } }
                HeadCell { text: @tr("Location"); column: 1; horizontal-stretch: 1; preferred-width: 0; sorted: -1; ascending: false; clicked => { root.header(1); } }
                HeadCell { text: @tr("Size"); column: 0; width: 90px; sorted: root.sorted; ascending: root.ascending; clicked => { root.header(0); } }
                HeadCell { text: @tr("Modified"); column: 3; width: 130px; sorted: root.sorted; ascending: root.ascending; clicked => { root.header(3); } }
            }
            Rule {}
            ListView {
                vertical-stretch: 1;
                for r in root.rows : Rectangle {
                    height: 24px;
                    background: list-touch.has-hover ? Theme.hover : transparent;
                    list-touch := TouchArea {
                        mouse-cursor: r.id >= 0 ? MouseCursor.pointer : MouseCursor.default;
                        clicked => {
                            if (r.id >= 0) {
                                root.select(r.id);
                            }
                        }
                    }
                    HorizontalLayout {
                        spacing: 8px;
                        Rectangle {
                            width: 16px;
                            Rectangle { width: 8px; height: 8px; background: r.color; }
                        }
                        Text { width: 200px; text: r.name; color: Theme.text; vertical-alignment: center; overflow: elide; }
                        Text { horizontal-stretch: 1; preferred-width: 0; text: r.detail; color: Theme.muted; vertical-alignment: center; overflow: elide; }
                        Text { width: 90px; text: r.size; color: Theme.amber; horizontal-alignment: right; vertical-alignment: center; }
                        Text { width: 130px; text: r.extra; color: Theme.dim; horizontal-alignment: right; vertical-alignment: center; }
                    }
                }
            }
        }
    }

    export component MainWindow inherits Window {
        title: @tr("Reclaim");
        preferred-width: 1120px;
        preferred-height: 780px;
        min-width: 720px;
        min-height: 520px;
        background: Theme.bg;
        default-font-family: "Consolas";
        default-font-size: 13px;
        in property <image> app-icon;
        icon: root.app-icon;

        init => {
            Palette.color-scheme = ColorScheme.dark;
        }

        // Navigation.
        in-out property <int> page: 0;
        callback navigate(int);

        // Warning banner.
        in property <string> warning;
        callback dismiss-warning();

        // Overview.
        in property <string> used;
        in property <string> total;
        in property <string> load;
        in property <string> committed;
        in property <int> used-permille;
        in property <int> commit-permille;
        in property <string> pressure;
        in property <bool> pressure-high;
        in property <string> standby;
        in property <string> modified;
        in property <string> paging;
        in property <bool> can-run: true;
        in property <bool> trim-supported: true;
        in-out property <bool> details: false;
        callback run();
        callback toggle-details(bool);

        // Processes. Table column 0 is the selection mark; data columns start at 1.
        in property <[ProcRow]> rows;
        in property <int> selected-count;
        in property <int> sort-column: 3;
        in property <bool> sort-ascending: false;
        in-out property <bool> eligible-only: false;
        callback sort(int, bool);
        callback toggle-row(int);
        callback set-eligible-only(bool);
        callback run-selected();

        // Confirmation.
        in property <bool> plan-open;
        in property <string> plan-title;
        in property <string> plan-skipped;
        in property <string> plan-caveat;
        in property <bool> plan-can-start;
        callback confirm();
        callback dismiss();

        // Status bar: run progress or the last trim's totals.
        in property <string> status;
        in property <bool> can-cancel;
        callback cancel-run();
        callback clear-history();

        // Settings.
        in-out property <string> s-threshold;
        in-out property <string> s-exclusions;
        in-out property <string> s-caution;
        in-out property <bool> s-confirm;
        in-out property <bool> s-history;
        in-out property <bool> s-log;
        in-out property <bool> s-autostart;
        in property <string> settings-message;
        in property <string> version;
        callback save-settings();
        callback revert-settings();

        // Storage: disk usage (page 4).
        in property <[string]> st-drive-titles;
        in property <[DriveItem]> st-drives;
        in-out property <int> st-drive-index: 0;
        in-out property <string> st-folders;
        in property <string> st-status;
        in property <bool> st-scanning;
        in property <bool> st-paused;
        in property <bool> st-can-pause;
        in property <bool> st-has-index;
        in property <bool> st-partial;
        in-out property <bool> st-watch;
        in property <[TreeRow]> st-tree;
        in property <int> st-tree-scroll: -1;
        in property <int> st-tree-serial;
        in property <[Tile]> st-tiles;
        in property <bool> st-has-sel;
        in property <float> st-sel-x;
        in property <float> st-sel-y;
        in property <float> st-sel-w;
        in property <float> st-sel-h;
        in property <string> st-sel-path;
        in property <string> st-sel-detail;
        in property <bool> st-can-recycle;
        in property <bool> st-can-refresh;
        in property <string> st-zoom-path;
        in property <bool> st-can-back;
        in property <bool> st-can-forward;
        in property <bool> st-can-up;
        in property <bool> st-icicle;
        in property <int> st-tab;
        in property <[ListRow]> st-list;
        in property <int> st-list-sort;
        in property <bool> st-list-asc;
        in property <string> st-search-message;
        in-out property <string> st-q-name;
        in-out property <string> st-q-path;
        in-out property <string> st-q-ext;
        in-out property <string> st-q-min;
        in-out property <string> st-q-max;
        in-out property <string> st-q-older;
        in-out property <string> st-q-newer;
        in-out property <int> st-q-kind;
        callback st-scan-drive(int);
        callback st-scan-folders();
        callback st-browse();
        callback st-pause(bool);
        callback st-cancel();
        callback st-refresh();
        callback st-export(bool);
        callback st-set-watch(bool);
        callback st-select(int);
        callback st-toggle(int);
        callback st-more(int);
        callback st-zoom-node(int);
        callback st-map-resized(float, float);
        pure callback st-map-hover(float, float) -> string;
        callback st-map-click(float, float);
        callback st-map-zoom(float, float);
        callback st-back();
        callback st-forward();
        callback st-up();
        callback st-set-icicle(bool);
        callback st-open-explorer();
        callback st-properties();
        callback st-recycle();
        callback st-set-tab(int);
        callback st-sort(int, bool);
        callback st-search();
        callback st-clear-search();

        // Storage: cleanup (page 5).
        in property <[CatRow]> cl-categories;
        in property <string> cl-detail;
        in property <[CandRow]> cl-candidates;
        in property <bool> cl-per-file;
        in property <string> cl-status;
        in property <bool> cl-busy;
        in property <int> cl-progress;
        in property <bool> cl-can-clean;
        in property <string> cl-report;
        in property <[string]> cl-history;
        callback cl-examine();
        callback cl-clean();
        callback cl-cancel();
        callback cl-toggle(string);
        callback cl-focus(string);
        callback cl-toggle-file(int);
        callback cl-clear-history();

        // Storage: duplicates (page 6).
        in property <[DupRow]> du-rows;
        in property <string> du-status;
        in property <bool> du-busy;
        in property <int> du-progress;
        in property <bool> du-can-find;
        in property <bool> du-can-recycle;
        callback du-find();
        callback du-cancel();
        callback du-toggle(int, int);
        callback du-open(int, int);
        callback du-recycle();

        // Storage confirmation.
        in property <bool> sc-open;
        in property <string> sc-title;
        in property <string> sc-body;
        in property <string> sc-ok;
        in property <bool> sc-danger;
        callback sc-confirm();
        callback sc-dismiss();

        // Storage settings.
        in-out property <string> ss-scan-excl;
        in-out property <string> ss-clean-excl;
        in-out property <string> ss-threads;
        in-out property <bool> ss-fast;
        in-out property <bool> ss-allocated;
        in-out property <string> ss-dup-min;
        in-out property <bool> ss-history;
        in property <bool> ss-fast-available;
        in property <string> ss-message;
        callback ss-save();
        callback ss-revert();

        // Text tabs on the left (page numbers in `tab-pages`); Settings (2) and About (3)
        // are icons on the right.
        private property <[string]> tabs: [@tr("Overview"), @tr("Processes"), @tr("Disk usage"), @tr("Cleanup"), @tr("Duplicates")];
        private property <[int]> tab-pages: [0, 1, 4, 5, 6];

        // Sorting from a header click: a new column starts in its natural order.
        function header-clicked(column: int) {
            if (column == root.sort-column) {
                root.sort(column, !root.sort-ascending);
            } else {
                root.sort(column, column == 1 || column == 2 || column == 5);
            }
        }

        function go(i: int) {
            root.page = i;
            root.navigate(i);
        }

        GridBackground {}

        VerticalLayout {
            spacing: 0;
            VerticalLayout {
                padding: 16px;
                padding-top: 4px;
                spacing: 14px;
                vertical-stretch: 1;

                // Tab strip.
                VerticalLayout {
                    spacing: 0;
                    HorizontalLayout {
                        spacing: 0;
                        for t[i] in root.tabs : TabItem {
                            text: t;
                            active: root.page == root.tab-pages[i];
                            clicked => { root.go(root.tab-pages[i]); }
                        }
                        Rectangle { horizontal-stretch: 1; }
                        IconTab {
                            tip: @tr("Settings");
                            active: root.page == 2;
                            commands: "M 19.27 9.78 L 21.78 9.92 L 21.78 14.08 L 19.27 14.22 L 18.71 15.57 L 20.39 17.45 L 17.45 20.39 L 15.57 18.71 L 14.22 19.27 L 14.08 21.78 L 9.92 21.78 L 9.78 19.27 L 8.43 18.71 L 6.55 20.39 L 3.61 17.45 L 5.29 15.57 L 4.73 14.22 L 2.22 14.08 L 2.22 9.92 L 4.73 9.78 L 5.29 8.43 L 3.61 6.55 L 6.55 3.61 L 8.43 5.29 L 9.78 4.73 L 9.92 2.22 L 14.08 2.22 L 14.22 4.73 L 15.57 5.29 L 17.45 3.61 L 20.39 6.55 L 18.71 8.43 Z M 15.2 12 A 3.2 3.2 0 1 1 8.8 12 A 3.2 3.2 0 1 1 15.2 12 Z";
                            clicked => { root.go(2); }
                        }
                        // About is always the rightmost icon.
                        IconTab {
                            tip: @tr("About");
                            active: root.page == 3;
                            commands: "M 21.5 12 A 9.5 9.5 0 1 1 2.5 12 A 9.5 9.5 0 1 1 21.5 12 Z M 12 10.5 L 12 17 M 12 7 L 12 8";
                            clicked => { root.go(3); }
                        }
                    }
                    Rule {}
                }

                if root.warning != "" : Rectangle {
                    background: #241c10;
                    border-width: 1px;
                    border-color: Theme.warn;
                    border-radius: 4px;
                    HorizontalLayout {
                        padding: 8px;
                        padding-left: 12px;
                        spacing: 12px;
                        Text {
                            text: root.warning;
                            color: Theme.warn;
                            vertical-alignment: center;
                            wrap: word-wrap;
                            horizontal-stretch: 1;
                        }
                        FlatButton { text: @tr("Dismiss"); clicked => { root.dismiss-warning(); } }
                    }
                }

                // Overview.
                if root.page == 0 : VerticalLayout {
                    alignment: start;
                    spacing: 14px;
                    Panel {
                        VerticalLayout {
                            padding: 20px;
                            spacing: 12px;
                            HorizontalLayout {
                                Caption { label: @tr("Used"); }
                                Bracket { value: @tr("{} of {}", root.used, root.total); }
                            }
                            Bar { permille: root.used-permille; fill: Theme.usage; }
                            Rectangle { height: 6px; }
                            HorizontalLayout {
                                Caption { label: @tr("Commit"); }
                                Bracket { value: root.committed; }
                            }
                            Bar { permille: root.commit-permille; fill: Theme.accent-dim; }
                            Rectangle { height: 4px; }
                            Rule {}
                            Value { label: @tr("Physical memory"); value: root.load; }
                            Value {
                                label: @tr("Memory pressure");
                                value: root.pressure;
                                value-color: root.pressure-high ? Theme.warn : Theme.text;
                            }
                        }
                    }
                    CheckBox {
                        text: @tr("Show memory details (standby, modified, paging)");
                        checked <=> root.details;
                        toggled => { root.toggle-details(self.checked); }
                    }
                    if root.details : Panel {
                        VerticalLayout {
                            padding: 20px;
                            padding-top: 14px;
                            padding-bottom: 14px;
                            spacing: 10px;
                            Value { label: @tr("Standby list"); value: root.standby; }
                            Value { label: @tr("Modified list"); value: root.modified; }
                            Value { label: @tr("Pages read from disk"); value: root.paging; }
                        }
                    }
                    if !root.trim-supported : Text {
                        text: @tr("Trimming is not available on this system.");
                        color: Theme.error;
                    }
                    HorizontalLayout {
                        alignment: start;
                        spacing: 10px;
                        FlatButton {
                            text: @tr("Trim working sets…");
                            primary: true;
                            enabled: root.can-run;
                            clicked => { root.run(); }
                        }
                    }
                }

                // Processes.
                if root.page == 1 : VerticalLayout {
                    spacing: 12px;
                    HorizontalLayout {
                        alignment: start;
                        spacing: 14px;
                        CheckBox {
                            text: @tr("Eligible only");
                            checked <=> root.eligible-only;
                            toggled => { root.set-eligible-only(self.checked); }
                        }
                        FlatButton {
                            text: @tr("Trim selected ({})…", root.selected-count);
                            primary: true;
                            enabled: root.can-run && root.selected-count > 0;
                            clicked => { root.run-selected(); }
                        }
                        Text {
                            text: @tr("Click a row to select it.");
                            color: Theme.dim;
                            vertical-alignment: center;
                        }
                    }
                    Panel {
                        vertical-stretch: 1;
                        VerticalLayout {
                            padding-left: 14px;
                            padding-right: 14px;
                            padding-bottom: 4px;
                            HorizontalLayout {
                                spacing: 12px;
                                Rectangle { width: Theme.col-mark; }
                                HeadCell {
                                    text: @tr("Process"); column: 1; horizontal-stretch: 1; preferred-width: 0;
                                    sorted: root.sort-column; ascending: root.sort-ascending;
                                    clicked => { root.header-clicked(1); }
                                }
                                HeadCell {
                                    text: @tr("PID"); column: 2; width: Theme.col-pid;
                                    sorted: root.sort-column; ascending: root.sort-ascending;
                                    clicked => { root.header-clicked(2); }
                                }
                                HeadCell {
                                    text: @tr("Working set"); column: 3; width: Theme.col-bytes;
                                    sorted: root.sort-column; ascending: root.sort-ascending;
                                    clicked => { root.header-clicked(3); }
                                }
                                HeadCell {
                                    text: @tr("Private bytes"); column: 4; width: Theme.col-bytes;
                                    sorted: root.sort-column; ascending: root.sort-ascending;
                                    clicked => { root.header-clicked(4); }
                                }
                                HeadCell {
                                    text: @tr("Trim?"); column: 5; horizontal-stretch: 1; preferred-width: 0;
                                    sorted: root.sort-column; ascending: root.sort-ascending;
                                    clicked => { root.header-clicked(5); }
                                }
                            }
                            Rule {}
                            ListView {
                                vertical-stretch: 1;
                                for r[i] in root.rows : Rectangle {
                                    height: 34px;
                                    background: row-touch.has-hover ? Theme.hover : transparent;
                                    HorizontalLayout {
                                        spacing: 12px;
                                        Text {
                                            width: Theme.col-mark;
                                            text: r.selected ? "✓" : "";
                                            color: Theme.accent;
                                            vertical-alignment: center;
                                        }
                                        Text {
                                            horizontal-stretch: 1;
                                            preferred-width: 0;
                                            text: r.name;
                                            color: r.selected ? Theme.accent : Theme.text;
                                            vertical-alignment: center;
                                            overflow: elide;
                                        }
                                        Text {
                                            width: Theme.col-pid;
                                            text: r.pid;
                                            color: Theme.muted;
                                            vertical-alignment: center;
                                        }
                                        Text {
                                            width: Theme.col-bytes;
                                            text: "[ " + r.working-set + " ]";
                                            color: Theme.text;
                                            vertical-alignment: center;
                                            overflow: elide;
                                        }
                                        Text {
                                            width: Theme.col-bytes;
                                            text: r.private-bytes;
                                            color: Theme.muted;
                                            vertical-alignment: center;
                                            overflow: elide;
                                        }
                                        Text {
                                            horizontal-stretch: 1;
                                            preferred-width: 0;
                                            text: r.eligible ? @tr("yes") : r.status;
                                            color: r.eligible ? Theme.accent : Theme.muted;
                                            font-weight: r.eligible ? 700 : 400;
                                            vertical-alignment: center;
                                            overflow: elide;
                                        }
                                    }
                                    Rectangle {
                                        y: parent.height - 1px;
                                        height: 1px;
                                        background: Theme.line;
                                    }
                                    row-touch := TouchArea {
                                        mouse-cursor: pointer;
                                        clicked => { root.toggle-row(i); }
                                    }
                                }
                            }
                        }
                    }
                }

                // Settings: memory on the left, storage on the right.
                if root.page == 2 : HorizontalLayout {
                  spacing: 24px;
                  VerticalLayout {
                    alignment: start;
                    spacing: 10px;
                    horizontal-stretch: 1;
                    Caption { label: @tr("Memory"); font-size: 12px; }
                    HorizontalLayout {
                        spacing: 12px;
                        Text {
                            text: @tr("Skip processes smaller than (MiB)");
                            color: Theme.muted;
                            vertical-alignment: center;
                            horizontal-stretch: 0;
                        }
                        LineEdit { text <=> root.s-threshold; input-type: number; max-width: 120px; }
                    }
                    Text { text: @tr("Never trim (one image name per line, for example game.exe)"); color: Theme.muted; }
                    TextEdit { text <=> root.s-exclusions; min-height: 70px; }
                    Text { text: @tr("Allow protected images (one per line)"); color: Theme.muted; }
                    TextEdit { text <=> root.s-caution; min-height: 50px; }
                    CheckBox { text: @tr("Ask before every run"); checked <=> root.s-confirm; }
                    CheckBox { text: @tr("Keep run history on this computer"); checked <=> root.s-history; }
                    CheckBox { text: @tr("Write errors to a local log file"); checked <=> root.s-log; }
                    CheckBox { text: @tr("Start Reclaim with Windows (in the tray)"); checked <=> root.s-autostart; }
                    Text { text: @tr("Changes take effect when you press Save."); color: Theme.dim; font-size: 11px; }
                    HorizontalLayout {
                        alignment: start;
                        spacing: 10px;
                        FlatButton { text: @tr("Save"); primary: true; clicked => { root.save-settings(); } }
                        FlatButton { text: @tr("Revert"); clicked => { root.revert-settings(); } }
                        FlatButton { text: @tr("Clear history and logs"); clicked => { root.clear-history(); } }
                        Text { text: root.settings-message; color: Theme.muted; vertical-alignment: center; }
                    }
                  }
                  VerticalLayout {
                    alignment: start;
                    spacing: 10px;
                    horizontal-stretch: 1;
                    Caption { label: @tr("Storage"); font-size: 12px; }
                    Text { text: @tr("Never scan (paths or names such as node_modules, one per line)"); color: Theme.muted; wrap: word-wrap; }
                    TextEdit { text <=> root.ss-scan-excl; min-height: 60px; }
                    Text { text: @tr("Never clean (paths or names, one per line)"); color: Theme.muted; wrap: word-wrap; }
                    TextEdit { text <=> root.ss-clean-excl; min-height: 50px; }
                    HorizontalLayout {
                        spacing: 12px;
                        Text { text: @tr("Scan threads (0 = automatic per drive)"); color: Theme.muted; vertical-alignment: center; }
                        LineEdit { text <=> root.ss-threads; input-type: number; max-width: 80px; }
                    }
                    HorizontalLayout {
                        spacing: 12px;
                        Text { text: @tr("Duplicates: ignore files smaller than (KiB)"); color: Theme.muted; vertical-alignment: center; }
                        LineEdit { text <=> root.ss-dup-min; input-type: number; max-width: 100px; }
                    }
                    CheckBox { text: @tr("Show sizes on disk (otherwise file lengths)"); checked <=> root.ss-allocated; }
                    CheckBox { text: @tr("Fast NTFS scan of whole drives"); checked <=> root.ss-fast; }
                    Text {
                        text: root.ss-fast-available
                            ? @tr("Available: Reclaim is running as administrator.")
                            : @tr("Used only when you start Reclaim as administrator yourself; Reclaim never asks for elevation. Otherwise drives are scanned folder by folder.");
                        color: Theme.dim;
                        font-size: 11px;
                        wrap: word-wrap;
                    }
                    CheckBox { text: @tr("Keep cleanup history on this computer (no file names)"); checked <=> root.ss-history; }
                    HorizontalLayout {
                        alignment: start;
                        spacing: 10px;
                        FlatButton { text: @tr("Save"); primary: true; clicked => { root.ss-save(); } }
                        FlatButton { text: @tr("Revert"); clicked => { root.ss-revert(); } }
                        Text { text: root.ss-message; color: Theme.muted; vertical-alignment: center; wrap: word-wrap; }
                    }
                  }
                }

                // Disk usage.
                if root.page == 4 : VerticalLayout {
                    spacing: 8px;
                    HorizontalLayout {
                        spacing: 10px;
                        ComboBox {
                            model: root.st-drive-titles;
                            current-index <=> root.st-drive-index;
                            min-width: 260px;
                            horizontal-stretch: 0;
                            enabled: !root.st-scanning;
                        }
                        FlatButton {
                            text: @tr("Scan drive");
                            primary: true;
                            enabled: !root.st-scanning && root.st-drive-titles.length > 0;
                            clicked => { root.st-scan-drive(root.st-drive-index); }
                        }
                        LineEdit {
                            text <=> root.st-folders;
                            placeholder-text: @tr("Folders to scan, separated by ;");
                            horizontal-stretch: 1;
                            enabled: !root.st-scanning;
                            accepted => { root.st-scan-folders(); }
                        }
                        FlatButton { text: @tr("Browse…"); enabled: !root.st-scanning; clicked => { root.st-browse(); } }
                        FlatButton { text: @tr("Scan folders"); enabled: !root.st-scanning && root.st-folders != ""; clicked => { root.st-scan-folders(); } }
                    }
                    HorizontalLayout {
                        spacing: 10px;
                        if root.st-scanning && root.st-can-pause : FlatButton {
                            text: root.st-paused ? @tr("Resume") : @tr("Pause");
                            clicked => { root.st-pause(!root.st-paused); }
                        }
                        if root.st-scanning : FlatButton { text: @tr("Cancel scan"); clicked => { root.st-cancel(); } }
                        FlatButton {
                            text: @tr("Refresh");
                            enabled: root.st-has-index && !root.st-scanning;
                            clicked => { root.st-refresh(); }
                        }
                        FlatButton { text: @tr("Export JSON…"); enabled: root.st-has-index && !root.st-scanning; clicked => { root.st-export(false); } }
                        FlatButton { text: @tr("Export CSV…"); enabled: root.st-has-index && !root.st-scanning; clicked => { root.st-export(true); } }
                        CheckBox {
                            text: @tr("Watch for changes");
                            checked <=> root.st-watch;
                            enabled: root.st-has-index;
                            toggled => { root.st-set-watch(self.checked); }
                        }
                        Text {
                            text: root.st-status;
                            color: root.st-partial ? Theme.warn : Theme.muted;
                            font-size: 12px;
                            vertical-alignment: center;
                            horizontal-stretch: 1;
                            overflow: elide;
                        }
                    }
                    HorizontalLayout {
                        spacing: 10px;
                        vertical-stretch: 1;
                        TreeView {
                            horizontal-stretch: 1;
                            rows: root.st-tree;
                            scroll-to: root.st-tree-scroll;
                            scroll-serial: root.st-tree-serial;
                            select(id) => { root.st-select(id); }
                            toggle(id) => { root.st-toggle(id); }
                            more(id) => { root.st-more(id); }
                            zoom(id) => { root.st-zoom-node(id); }
                        }
                        VerticalLayout {
                            horizontal-stretch: 1;
                            spacing: 6px;
                            HorizontalLayout {
                                spacing: 8px;
                                Link { text: "‹ " + @tr("Back"); enabled: root.st-can-back; clicked => { root.st-back(); } }
                                Link { text: @tr("Forward") + " ›"; enabled: root.st-can-forward; clicked => { root.st-forward(); } }
                                Link { text: "↑ " + @tr("Up"); enabled: root.st-can-up; clicked => { root.st-up(); } }
                                Text {
                                    text: root.st-zoom-path;
                                    color: Theme.muted;
                                    font-size: 12px;
                                    vertical-alignment: center;
                                    horizontal-stretch: 1;
                                    overflow: elide;
                                }
                                Link { text: @tr("Treemap"); color: root.st-icicle ? Theme.dim : Theme.accent; clicked => { root.st-set-icicle(false); } }
                                Link { text: @tr("Icicle"); color: root.st-icicle ? Theme.accent : Theme.dim; clicked => { root.st-set-icicle(true); } }
                            }
                            MapView {
                                vertical-stretch: 1;
                                tiles: root.st-tiles;
                                has-sel: root.st-has-sel;
                                sel-x: root.st-sel-x;
                                sel-y: root.st-sel-y;
                                sel-w: root.st-sel-w;
                                sel-h: root.st-sel-h;
                                resized(w, h) => { root.st-map-resized(w, h); }
                                hover(x, y) => { return root.st-map-hover(x, y); }
                                click(x, y) => { root.st-map-click(x, y); }
                                zoom(x, y) => { root.st-map-zoom(x, y); }
                            }
                            Text {
                                text: @tr("Click to select, double-click to zoom in. Colours follow the file types in Extensions.");
                                color: Theme.dim;
                                font-size: 11px;
                            }
                        }
                    }
                    if root.st-has-sel : Panel {
                        HorizontalLayout {
                            padding: 10px;
                            spacing: 12px;
                            VerticalLayout {
                                horizontal-stretch: 1;
                                spacing: 2px;
                                Text { text: root.st-sel-path; color: Theme.accent; overflow: elide; }
                                Text { text: root.st-sel-detail; color: Theme.muted; font-size: 12px; wrap: word-wrap; }
                            }
                            VerticalLayout {
                                alignment: center;
                                HorizontalLayout {
                                    spacing: 8px;
                                    FlatButton { text: @tr("Open in Explorer"); clicked => { root.st-open-explorer(); } }
                                    FlatButton { text: @tr("Properties"); clicked => { root.st-properties(); } }
                                    FlatButton { text: @tr("Rescan folder"); enabled: root.st-can-refresh; clicked => { root.st-refresh(); } }
                                    FlatButton { text: @tr("Recycle…"); enabled: root.st-can-recycle; clicked => { root.st-recycle(); } }
                                }
                            }
                        }
                    }
                    VerticalLayout {
                        spacing: 4px;
                        HorizontalLayout {
                            spacing: 0;
                            for t[i] in [@tr("Largest files"), @tr("Largest folders"), @tr("Extensions"), @tr("Search"), @tr("Errors")] : TabItem {
                                text: t;
                                active: root.st-tab == i;
                                clicked => { root.st-set-tab(i); }
                            }
                            Rectangle { horizontal-stretch: 1; }
                        }
                        if root.st-tab == 3 : HorizontalLayout {
                            spacing: 6px;
                            LineEdit { text <=> root.st-q-name; placeholder-text: @tr("Name, *.log or /regex/"); horizontal-stretch: 2; accepted => { root.st-search(); } }
                            LineEdit { text <=> root.st-q-path; placeholder-text: @tr("Path contains"); horizontal-stretch: 2; accepted => { root.st-search(); } }
                            LineEdit { text <=> root.st-q-ext; placeholder-text: @tr("Extensions"); horizontal-stretch: 1; accepted => { root.st-search(); } }
                            LineEdit { text <=> root.st-q-min; placeholder-text: @tr("Min size"); horizontal-stretch: 1; accepted => { root.st-search(); } }
                            LineEdit { text <=> root.st-q-max; placeholder-text: @tr("Max size"); horizontal-stretch: 1; accepted => { root.st-search(); } }
                            LineEdit { text <=> root.st-q-older; placeholder-text: @tr("Older than days"); horizontal-stretch: 1; accepted => { root.st-search(); } }
                            LineEdit { text <=> root.st-q-newer; placeholder-text: @tr("Newer than days"); horizontal-stretch: 1; accepted => { root.st-search(); } }
                            ComboBox { model: [@tr("Any"), @tr("Files"), @tr("Folders")]; current-index <=> root.st-q-kind; horizontal-stretch: 0; }
                            FlatButton { text: @tr("Find"); primary: true; enabled: root.st-has-index; clicked => { root.st-search(); } }
                            FlatButton { text: @tr("Clear"); clicked => { root.st-clear-search(); } }
                        }
                        if root.st-tab == 3 : Text { text: root.st-search-message; color: Theme.muted; font-size: 12px; }
                        ResultList {
                            height: 180px;
                            rows: root.st-list;
                            sortable: root.st-tab == 0 || root.st-tab == 1 || root.st-tab == 3;
                            sorted: self.sortable ? root.st-list-sort : -1;
                            ascending: root.st-list-asc;
                            sort(c, asc) => { root.st-sort(c, asc); }
                            select(id) => { root.st-select(id); }
                        }
                    }
                }

                // Cleanup.
                if root.page == 5 : VerticalLayout {
                    spacing: 10px;
                    HorizontalLayout {
                        spacing: 10px;
                        FlatButton { text: @tr("Examine"); primary: !root.cl-can-clean; enabled: !root.cl-busy; clicked => { root.cl-examine(); } }
                        FlatButton { text: @tr("Clean selected…"); primary: root.cl-can-clean; enabled: root.cl-can-clean; clicked => { root.cl-clean(); } }
                        if root.cl-busy : FlatButton { text: @tr("Cancel"); clicked => { root.cl-cancel(); } }
                        Text {
                            text: root.cl-status;
                            color: Theme.muted;
                            vertical-alignment: center;
                            horizontal-stretch: 1;
                            overflow: elide;
                        }
                    }
                    if root.cl-busy && root.cl-progress > 0 : Bar { permille: root.cl-progress; }
                    HorizontalLayout {
                        spacing: 10px;
                        vertical-stretch: 1;
                        Panel {
                            horizontal-stretch: 1;
                            VerticalLayout {
                                padding: 8px;
                                ListView {
                                    for c in root.cl-categories : Rectangle {
                                        height: 46px;
                                        background: c.focused ? #3fe3f214 : cat-touch.has-hover ? Theme.hover : transparent;
                                        cat-touch := TouchArea {
                                            mouse-cursor: pointer;
                                            clicked => { root.cl-focus(c.id); }
                                        }
                                        HorizontalLayout {
                                            padding-left: 6px;
                                            padding-right: 6px;
                                            spacing: 10px;
                                            Text {
                                                width: 18px;
                                                text: c.selected ? "☑" : "☐";
                                                font-size: 15px;
                                                color: !c.enabled ? Theme.dim : c.selected ? Theme.accent : Theme.muted;
                                                vertical-alignment: center;
                                                TouchArea {
                                                    enabled: c.enabled;
                                                    mouse-cursor: pointer;
                                                    clicked => { root.cl-toggle(c.id); }
                                                }
                                            }
                                            VerticalLayout {
                                                horizontal-stretch: 1;
                                                alignment: center;
                                                Text { text: c.name; color: c.enabled ? Theme.text : Theme.muted; overflow: elide; }
                                                Text {
                                                    text: c.safety + (c.files == "" ? "" : "  ·  " + c.files);
                                                    font-size: 11px;
                                                    color: c.level == 0 ? Theme.accent-dim : c.level == 1 ? Theme.amber : Theme.warn;
                                                    overflow: elide;
                                                }
                                            }
                                            Text { text: c.size; color: Theme.amber; vertical-alignment: center; horizontal-alignment: right; }
                                        }
                                        Rectangle { y: parent.height - 1px; height: 1px; background: Theme.line; }
                                    }
                                }
                            }
                        }
                        Panel {
                            horizontal-stretch: 1;
                            VerticalLayout {
                                padding: 12px;
                                spacing: 8px;
                                Text {
                                    text: root.cl-detail == "" ? @tr("Select a category to see what it contains, what happens after cleaning and how it is removed.") : root.cl-detail;
                                    color: root.cl-detail == "" ? Theme.dim : Theme.text;
                                    wrap: word-wrap;
                                    vertical-stretch: root.cl-per-file ? 0 : 1;
                                    vertical-alignment: top;
                                }
                                if root.cl-per-file : Text { text: @tr("Choose files one by one:"); color: Theme.muted; font-size: 12px; }
                                if root.cl-per-file : ListView {
                                    vertical-stretch: 1;
                                    for f[i] in root.cl-candidates : Rectangle {
                                        height: 24px;
                                        background: cand-touch.has-hover ? Theme.hover : transparent;
                                        cand-touch := TouchArea { mouse-cursor: pointer; clicked => { root.cl-toggle-file(i); } }
                                        HorizontalLayout {
                                            spacing: 8px;
                                            Text { width: 16px; text: f.selected ? "☑" : "☐"; color: f.selected ? Theme.accent : Theme.muted; vertical-alignment: center; }
                                            Text { horizontal-stretch: 1; preferred-width: 0; text: f.path; color: Theme.text; overflow: elide; vertical-alignment: center; }
                                            Text { width: 90px; text: f.size; color: Theme.amber; horizontal-alignment: right; vertical-alignment: center; }
                                            Text { width: 120px; text: f.modified; color: Theme.dim; horizontal-alignment: right; vertical-alignment: center; }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if root.cl-report != "" : Panel {
                        border-color: Theme.accent-dim;
                        VerticalLayout {
                            padding: 12px;
                            Text { text: root.cl-report; color: Theme.text; font-size: 12px; wrap: word-wrap; }
                        }
                    }
                    HorizontalLayout {
                        spacing: 10px;
                        Caption { label: @tr("History"); font-size: 11px; }
                        Text {
                            text: root.cl-history.length == 0 ? @tr("No cleanup runs stored.") : root.cl-history[0];
                            color: Theme.dim;
                            font-size: 12px;
                            horizontal-stretch: 1;
                            overflow: elide;
                            vertical-alignment: center;
                        }
                        if root.cl-history.length > 0 : Link { text: @tr("Clear history"); clicked => { root.cl-clear-history(); } }
                    }
                }

                // Duplicates.
                if root.page == 6 : VerticalLayout {
                    spacing: 10px;
                    HorizontalLayout {
                        spacing: 10px;
                        FlatButton { text: @tr("Find duplicates"); primary: true; enabled: root.du-can-find; clicked => { root.du-find(); } }
                        if root.du-busy : FlatButton { text: @tr("Stop"); clicked => { root.du-cancel(); } }
                        FlatButton { text: @tr("Move marked to Recycle Bin…"); enabled: root.du-can-recycle; clicked => { root.du-recycle(); } }
                        Text { text: root.du-status; color: Theme.muted; wrap: word-wrap; horizontal-stretch: 1; vertical-alignment: center; }
                    }
                    if root.du-busy : Bar { permille: root.du-progress; }
                    Panel {
                        vertical-stretch: 1;
                        VerticalLayout {
                            padding: 8px;
                            ListView {
                                for d in root.du-rows : Rectangle {
                                    height: d.header ? 30px : 24px;
                                    background: !d.header && dup-touch.has-hover ? Theme.hover : transparent;
                                    dup-touch := TouchArea {
                                        enabled: !d.header;
                                        mouse-cursor: pointer;
                                        clicked => { root.du-toggle(d.group, d.file); }
                                    }
                                    HorizontalLayout {
                                        padding-left: d.header ? 0px : 18px;
                                        spacing: 8px;
                                        if !d.header : Text { width: 16px; text: d.marked ? "☑" : "☐"; color: d.marked ? Theme.warn : Theme.muted; vertical-alignment: center; }
                                        Text {
                                            horizontal-stretch: 1;
                                            preferred-width: 0;
                                            text: d.text;
                                            color: d.header ? Theme.accent : d.marked ? Theme.warn : Theme.text;
                                            font-weight: d.header ? 700 : 400;
                                            overflow: elide;
                                            vertical-alignment: center;
                                        }
                                        Text { text: d.size; color: d.header ? Theme.amber : Theme.dim; vertical-alignment: center; }
                                        if !d.header : Link { text: @tr("Show"); clicked => { root.du-open(d.group, d.file); } }
                                    }
                                }
                            }
                        }
                    }
                    Text {
                        text: @tr("Mark the copies you do not need. One copy of each group always stays, and marked files go to the Recycle Bin.");
                        color: Theme.dim;
                        font-size: 11px;
                        wrap: word-wrap;
                    }
                }

                // About.
                if root.page == 3 : VerticalLayout {
                    alignment: start;
                    spacing: 10px;
                    Text {
                        text: @tr("Reclaim {}", root.version).to-uppercase();
                        color: Theme.accent;
                        letter-spacing: Theme.spacing;
                    }
                    Text {
                        text: @tr("Trims the working sets of eligible processes on request and reports what changed, including what did not. Nothing leaves this computer.");
                        color: Theme.text;
                        wrap: word-wrap;
                    }
                    Text {
                        text: @tr("Licensed under the MIT license. Third-party notices are in THIRD_PARTY_NOTICES.");
                        color: Theme.muted;
                        wrap: word-wrap;
                    }
                    Rectangle { height: 8px; }
                    Rule {}
                    Text {
                        text: @tr("The window is drawn with Slint, a third-party UI toolkit used under its royalty-free license.");
                        color: Theme.dim;
                        font-size: 11px;
                        wrap: word-wrap;
                    }
                    // Attribution required by the Slint Royalty-free License 2.0, §2(a): the
                    // AboutSlint widget on the About screen (ADR-0001). Kept small on purpose.
                    HorizontalLayout {
                        alignment: start;
                        AboutSlint { width: 150px; height: 104px; }
                    }
                }
            }

            // Status bar.
            Rectangle {
                height: 28px;
                vertical-stretch: 0;
                background: Theme.panel;
                Rule { y: 0; width: parent.width; }
                HorizontalLayout {
                    padding-left: 16px;
                    padding-right: 16px;
                    spacing: 12px;
                    Text {
                        text: root.status;
                        font-size: 12px;
                        color: Theme.muted;
                        vertical-alignment: center;
                        overflow: elide;
                        horizontal-stretch: 1;
                    }
                    if root.can-cancel : Text {
                        text: @tr("Cancel").to-uppercase();
                        font-size: 12px;
                        letter-spacing: 1px;
                        color: cancel-touch.has-hover ? Theme.text : Theme.accent;
                        vertical-alignment: center;
                        cancel-touch := TouchArea {
                            mouse-cursor: pointer;
                            clicked => { root.cancel-run(); }
                        }
                    }
                }
            }
        }

        if root.plan-open : Rectangle {
            background: #000000a0;
            TouchArea {}
            Panel {
                width: min(parent.width - 40px, 480px);
                height: dialog.preferred-height;
                border-color: Theme.accent-dim;
                dialog := VerticalLayout {
                    padding: 20px;
                    spacing: 12px;
                    Text { text: root.plan-title; wrap: word-wrap; color: Theme.text; font-weight: 700; }
                    if root.plan-skipped != "" : Text { text: root.plan-skipped; wrap: word-wrap; color: Theme.muted; }
                    Text { text: root.plan-caveat; wrap: word-wrap; color: Theme.dim; }
                    HorizontalLayout {
                        alignment: end;
                        spacing: 10px;
                        if root.plan-can-start : FlatButton {
                            text: @tr("Start");
                            primary: true;
                            clicked => { root.confirm(); }
                        }
                        FlatButton {
                            text: root.plan-can-start ? @tr("Cancel") : @tr("Close");
                            clicked => { root.dismiss(); }
                        }
                    }
                }
            }
        }
        if root.sc-open : Rectangle {
            background: #000000a0;
            TouchArea {}
            Panel {
                width: min(parent.width - 40px, 560px);
                height: sc-dialog.preferred-height;
                border-color: root.sc-danger ? Theme.warn : Theme.accent-dim;
                sc-dialog := VerticalLayout {
                    padding: 20px;
                    spacing: 12px;
                    Text { text: root.sc-title; wrap: word-wrap; color: Theme.text; font-weight: 700; }
                    Text { text: root.sc-body; wrap: word-wrap; color: Theme.muted; }
                    HorizontalLayout {
                        alignment: end;
                        spacing: 10px;
                        FlatButton { text: root.sc-ok; primary: true; clicked => { root.sc-confirm(); } }
                        FlatButton { text: @tr("Cancel"); clicked => { root.sc-dismiss(); } }
                    }
                }
            }
        }
    }

    export component Tray inherits SystemTrayIcon {
        in property <image> tray-icon;
        in property <string> tip;
        in property <bool> can-run: true;
        icon: root.tray-icon;
        tooltip: root.tip;
        callback open();
        callback run();
        callback quit();
        clicked => { root.open(); }
        Menu {
            MenuItem { title: @tr("Open Reclaim"); activated => { root.open(); } }
            MenuItem { title: @tr("Trim working sets…"); enabled: root.can-run; activated => { root.run(); } }
            MenuSeparator {}
            MenuItem { title: @tr("Quit"); activated => { root.quit(); } }
        }
    }
}
