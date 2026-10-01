import { App, PluginSettingTab, Setting } from "obsidian";
import { DEFAULT_DONE_HEADING } from "./markdown";
import type RestaskPlugin from "./main";

/** Plugin settings (§15.4). */
export interface RestaskSettings {
	/** Heading that starts the completed region; must equal the vault config `done_heading`. */
	doneHeading: string;
	/** Offer metadata suggestions while typing in a task line (§15.2). */
	suggestWhileTyping: boolean;
}

/** Defaults per §15.4. */
export const DEFAULT_SETTINGS: RestaskSettings = {
	doneHeading: DEFAULT_DONE_HEADING,
	suggestWhileTyping: true,
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
				"In a task line, typing two or more letters of a keyword (high, medium, low, due, start, scheduled, repeat, today, tomorrow) offers the matching metadata. Esc dismisses the menu.",
			)
			.addToggle((toggle) =>
				toggle.setValue(this.plugin.settings.suggestWhileTyping).onChange(async (value) => {
					this.plugin.settings.suggestWhileTyping = value;
					await this.plugin.saveSettings();
				}),
			);
	}
}
