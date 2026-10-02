import { App, PluginSettingTab, Setting } from "obsidian";
import { DEFAULT_DONE_HEADING } from "./markdown";
import type RestaskPlugin from "./main";

/** Plugin settings (§15.4). */
export interface RestaskSettings {
	/** Heading that starts the completed region; must equal the vault config `done_heading`. */
	doneHeading: string;
	/** Offer metadata suggestions while typing in a task line (§15.2). */
	suggestWhileTyping: boolean;
	/** Keep the `🆔` token of task lines off the screen (§15.5); it stays in the note. */
	hideTaskIds: boolean;
	/** Which devices make the daemon's local edits themselves instead of waiting for it (§15.6). */
	settleTasks: SettleTasks;
	/** Start a new line of a TODO section with a checkbox (§15.7). */
	startTasks: boolean;
}

/**
 * Values of `settleTasks`. The settings file rides the file sync, so one value has
 * to suit every device of the vault: `mobile` means "wherever the app runs as the mobile
 * app".
 */
export type SettleTasks = "mobile" | "always" | "never";

const SETTLE_TASKS: Readonly<Record<SettleTasks, string>> = {
	always: "On every device",
	mobile: "On phones and tablets only",
	never: "Never (leave it to the daemon)",
};

/** Defaults per §15.4. */
export const DEFAULT_SETTINGS: RestaskSettings = {
	doneHeading: DEFAULT_DONE_HEADING,
	suggestWhileTyping: true,
	hideTaskIds: true,
	settleTasks: "always",
	startTasks: true,
};

/** Settings tab exposing the §15.4 options. */
export class RestaskSettingTab extends PluginSettingTab {
	private readonly plugin: RestaskPlugin;

	constructor(app: App, plugin: RestaskPlugin) {
		super(app, plugin);
		this.plugin = plugin;
	}

	display(): void {
		const { containerEl } = this;
		containerEl.empty();

		new Setting(containerEl)
			.setName("Done heading")
			.setDesc("Heading that starts the completed region; must equal done_heading in restask.toml.")
			.addText((text) =>
				text.setValue(this.plugin.settings.doneHeading).onChange(async (value) => {
					this.plugin.settings.doneHeading = value.trim() === "" ? DEFAULT_DONE_HEADING : value.trim();
					await this.plugin.saveSettings();
				}),
			);

		new Setting(containerEl)
			.setName("Suggest while typing")
			.setDesc(
				"In a task line, typing two or more letters of a keyword (high, medium, low, due, start, scheduled, repeat, created, today, tomorrow) offers the matching metadata. Esc dismisses the menu.",
			)
			.addToggle((toggle) =>
				toggle.setValue(this.plugin.settings.suggestWhileTyping).onChange(async (value) => {
					this.plugin.settings.suggestWhileTyping = value;
					await this.plugin.saveSettings();
				}),
			);

		new Setting(containerEl)
			.setName("Hide task IDs")
			.setDesc(
				"Do not show the 🆔 token of task lines, in the editor and in reading view. The token stays in the note: it is what links the line to its task on the server. Turn this off to see it, for instance when a task does not sync.",
			)
			.addToggle((toggle) =>
				toggle.setValue(this.plugin.settings.hideTaskIds).onChange(async (value) => {
					this.plugin.settings.hideTaskIds = value;
					await this.plugin.saveSettings();
					this.plugin.applyConcealment();
				}),
			);

		new Setting(containerEl)
			.setName("Start new lines with a checkbox")
			.setDesc(
				"In a note restask syncs, a new line under a TODO heading starts with \"- [ ] \"; in the TODO view, every new line above Done does. Press Enter on the empty checkbox to take it away again.",
			)
			.addToggle((toggle) =>
				toggle.setValue(this.plugin.settings.startTasks).onChange(async (value) => {
					this.plugin.settings.startTasks = value;
					await this.plugin.saveSettings();
				}),
			);

		new Setting(containerEl)
			.setName("Apply task changes on the device")
			.setDesc(
				"Do at once, and offline, what the daemon does when a note reaches it: give a new task its creation date and ID, move a task you check off under the done heading (and back when you uncheck it), and keep the TODO view and the notes in step with each other. This setting is shared by all devices of the vault. If the daemon runs on the computer you also use Obsidian on and the two get in each other's way there, choose phones and tablets only — turning the plugin off instead would turn it off on your phone too.",
			)
			.addDropdown((dropdown) =>
				dropdown
					.addOptions(SETTLE_TASKS)
					.setValue(this.plugin.settings.settleTasks)
					.onChange(async (value) => {
						if (value in SETTLE_TASKS) this.plugin.settings.settleTasks = value as SettleTasks;
						await this.plugin.saveSettings();
					}),
			);
	}
}
