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
        CheckBox, LineEdit, TextEdit, ListView, Palette, AboutSlint,
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

    export component MainWindow inherits Window {
        title: @tr("Reclaim");
        preferred-width: 820px;
        preferred-height: 600px;
        min-width: 600px;
        min-height: 440px;
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

        // Text tabs on the left; Settings (2) and About (3) are icons on the right.
        private property <[string]> tabs: [@tr("Overview"), @tr("Processes")];

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
                            active: root.page == i;
                            clicked => { root.go(i); }
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

                // Settings.
                if root.page == 2 : VerticalLayout {
                    alignment: start;
                    spacing: 10px;
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
