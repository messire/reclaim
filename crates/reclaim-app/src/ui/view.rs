//! Slint markup for every MVP screen and the tray. Static labels use `@tr(...)` so a
//! translation catalogue can replace them without code changes (PRODUCT §7).

slint::slint! {
    #[edition = "1.19"]
    import {
        Button, CheckBox, LineEdit, TextEdit, StandardTableView, StandardListView,
        ProgressIndicator, ScrollView, GroupBox, VerticalBox, HorizontalBox, TabWidget,
        Palette, AboutSlint,
    } from "std-widgets.slint";

    export struct ReportLine {
        // 0 info, 1 warning, 2 cost, 3 note.
        tone: int,
        text: string,
    }

    component Value inherits HorizontalBox {
        in property <string> label;
        in property <string> value;
        padding: 2px;
        Text { text: root.label; min-width: 180px; color: Palette.foreground.transparentize(30%); }
        Text { text: root.value; font-weight: 600; }
    }

    export component MainWindow inherits Window {
        title: @tr("Reclaim");
        preferred-width: 820px;
        preferred-height: 600px;
        min-width: 560px;
        min-height: 420px;

        // Navigation.
        in-out property <int> page: 0;
        callback navigate(int);

        // Warning banner.
        in property <string> warning;
        callback dismiss-warning();

        // Overview.
        in property <string> available;
        in property <string> total;
        in property <string> load;
        in property <string> committed;
        in property <string> pressure;
        in property <string> standby;
        in property <string> modified;
        in property <string> paging;
        in property <string> last-run;
        in property <bool> can-run: true;
        in property <bool> trim-supported: true;
        in-out property <bool> details: false;
        callback run(bool);
        callback toggle-details(bool);

        // Processes.
        in property <[[StandardListViewItem]]> rows;
        in property <int> selected-count;
        in-out property <bool> eligible-only: false;
        callback sort(int, bool);
        callback toggle-row(int);
        callback set-eligible-only(bool);
        callback run-selected(bool);

        // Confirmation.
        in property <bool> plan-open;
        in property <string> plan-title;
        in property <string> plan-skipped;
        in property <string> plan-caveat;
        in property <bool> plan-can-start;
        callback confirm();
        callback dismiss();

        // Result.
        in property <string> result-status;
        in property <[ReportLine]> result-lines;
        in property <[[StandardListViewItem]]> result-rows;
        in property <bool> result-can-cancel;
        in property <float> result-progress;
        callback cancel-run();

        // History.
        in property <[StandardListViewItem]> history;
        callback clear-history();

        // Settings.
        in-out property <string> s-threshold;
        in-out property <string> s-exclusions;
        in-out property <string> s-caution;
        in-out property <bool> s-confirm;
        in-out property <bool> s-history;
        in-out property <bool> s-log;
        in property <string> settings-message;
        in property <string> version;
        callback save-settings();
        callback revert-settings();

        VerticalBox {
            if root.warning != "" : Rectangle {
                background: #fff4ce;
                border-radius: 4px;
                height: 36px;
                HorizontalBox {
                    padding: 4px;
                    Text { text: root.warning; color: #3d2f00; vertical-alignment: center; wrap: word-wrap; horizontal-stretch: 1; }
                    Button { text: @tr("Dismiss"); clicked => { root.dismiss-warning(); } }
                }
            }

            TabWidget {
                current-index <=> root.page;
                changed current-index => { root.navigate(self.current-index); }

                Tab {
                    title: @tr("Overview");
                    VerticalBox {
                        alignment: start;
                        GroupBox {
                            title: @tr("System memory");
                            VerticalBox {
                                Value { label: @tr("Available"); value: root.available; }
                                Value { label: @tr("Installed"); value: root.total; }
                                Value { label: @tr("Physical memory"); value: root.load; }
                                Value { label: @tr("Committed"); value: root.committed; }
                                Value { label: @tr("Memory pressure"); value: root.pressure; }
                            }
                        }
                        CheckBox {
                            text: @tr("Show memory details (standby, modified, paging)");
                            checked <=> root.details;
                            toggled => { root.toggle-details(self.checked); }
                        }
                        if root.details : GroupBox {
                            title: @tr("Memory details");
                            VerticalBox {
                                Value { label: @tr("Standby list"); value: root.standby; }
                                Value { label: @tr("Modified list"); value: root.modified; }
                                Value { label: @tr("Pages read from disk"); value: root.paging; }
                            }
                        }
                        Text { text: root.last-run; wrap: word-wrap; }
                        if !root.trim-supported : Text {
                            text: @tr("Trimming is not available on this system.");
                            color: #a4262c;
                        }
                        HorizontalBox {
                            alignment: start;
                            Button {
                                text: @tr("Trim working sets…");
                                primary: true;
                                enabled: root.can-run;
                                clicked => { root.run(false); }
                            }
                            Button {
                                text: @tr("Dry run…");
                                enabled: root.can-run;
                                clicked => { root.run(true); }
                            }
                        }
                    }
                }

                Tab {
                    title: @tr("Processes");
                    VerticalBox {
                        HorizontalBox {
                            alignment: start;
                            CheckBox {
                                text: @tr("Eligible only");
                                checked <=> root.eligible-only;
                                toggled => { root.set-eligible-only(self.checked); }
                            }
                            Button {
                                text: @tr("Select / unselect");
                                enabled: table.current-row >= 0;
                                clicked => { root.toggle-row(table.current-row); }
                            }
                            Button {
                                text: @tr("Trim selected ({})…", root.selected-count);
                                enabled: root.can-run && root.selected-count > 0;
                                clicked => { root.run-selected(false); }
                            }
                        }
                        table := StandardTableView {
                            columns: [
                                { title: "✓", width: 28px },
                                { title: @tr("Process") },
                                { title: @tr("PID") },
                                { title: @tr("Working set") },
                                { title: @tr("Private bytes") },
                                { title: @tr("Status") },
                            ];
                            rows: root.rows;
                            sort-ascending(c) => { root.sort(c, true); }
                            sort-descending(c) => { root.sort(c, false); }
                        }
                    }
                }

                Tab {
                    title: @tr("Result");
                    VerticalBox {
                        HorizontalBox {
                            Text { text: root.result-status; vertical-alignment: center; horizontal-stretch: 1; }
                            if root.result-can-cancel : Button {
                                text: @tr("Cancel");
                                clicked => { root.cancel-run(); }
                            }
                        }
                        if root.result-progress < 1.0 : ProgressIndicator { progress: root.result-progress; }
                        if root.result-lines.length == 0 && root.result-status == "" : Text {
                            text: @tr("Run a trim or a dry run to see its result here.");
                        }
                        ScrollView {
                            vertical-stretch: 1;
                            VerticalBox {
                                alignment: start;
                                for line in root.result-lines : Text {
                                    text: line.text;
                                    wrap: word-wrap;
                                    color: line.tone == 1 ? #a4262c : line.tone == 2 ? #8a5300 : line.tone == 3 ? Palette.foreground.transparentize(35%) : Palette.foreground;
                                }
                            }
                        }
                        StandardTableView {
                            vertical-stretch: 1;
                            columns: [{ title: @tr("Process") }, { title: @tr("Outcome") }];
                            rows: root.result-rows;
                        }
                    }
                }

                Tab {
                    title: @tr("History");
                    VerticalBox {
                        StandardListView { model: root.history; }
                        HorizontalBox {
                            alignment: start;
                            Button {
                                text: @tr("Clear history and logs");
                                clicked => { root.clear-history(); }
                            }
                        }
                    }
                }

                Tab {
                    title: @tr("Settings");
                    VerticalBox {
                        alignment: start;
                        HorizontalBox {
                            Text { text: @tr("Skip processes smaller than (MiB)"); vertical-alignment: center; }
                            LineEdit { text <=> root.s-threshold; input-type: number; max-width: 120px; }
                        }
                        Text { text: @tr("Never trim (one image name per line, for example game.exe)"); }
                        TextEdit { text <=> root.s-exclusions; min-height: 70px; }
                        Text { text: @tr("Allow protected images (one per line)"); }
                        TextEdit { text <=> root.s-caution; min-height: 50px; }
                        CheckBox { text: @tr("Ask before every run"); checked <=> root.s-confirm; }
                        CheckBox { text: @tr("Keep run history on this computer"); checked <=> root.s-history; }
                        CheckBox { text: @tr("Write errors to a local log file"); checked <=> root.s-log; }
                        HorizontalBox {
                            alignment: start;
                            Button { text: @tr("Save"); primary: true; clicked => { root.save-settings(); } }
                            Button { text: @tr("Revert"); clicked => { root.revert-settings(); } }
                            Text { text: root.settings-message; vertical-alignment: center; }
                        }
                    }
                }

                Tab {
                    title: @tr("About");
                    VerticalBox {
                        alignment: start;
                        Text { text: @tr("Reclaim {}", root.version); font-weight: 600; }
                        Text {
                            text: @tr("Trims the working sets of eligible processes on request and reports what changed, including what did not. Nothing leaves this computer.");
                            wrap: word-wrap;
                        }
                        Text { text: @tr("Licensed under the MIT license. Third-party notices are in THIRD_PARTY_NOTICES."); wrap: word-wrap; }
                        // Attribution required by the Slint Royalty-free License 2.0 (ADR-0001).
                        AboutSlint { max-height: 220px; }
                    }
                }
            }
        }

        if root.plan-open : Rectangle {
            background: #00000080;
            TouchArea {}
            Rectangle {
                width: min(parent.width - 40px, 460px);
                height: dialog.preferred-height;
                background: Palette.background;
                border-radius: 6px;
                dialog := VerticalBox {
                    Text { text: root.plan-title; wrap: word-wrap; font-weight: 600; }
                    if root.plan-skipped != "" : Text { text: root.plan-skipped; wrap: word-wrap; }
                    Text { text: root.plan-caveat; wrap: word-wrap; }
                    HorizontalBox {
                        alignment: end;
                        if root.plan-can-start : Button {
                            text: @tr("Start");
                            primary: true;
                            clicked => { root.confirm(); }
                        }
                        Button {
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
