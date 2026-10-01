import { App, PluginSettingTab, Setting } from "obsidian";
import { DEFAULT_DONE_HEADING } from "./markdown";
import type RestaskPlugin from "./main";

/** Plugin settings (§15.4). */
export interface RestaskSettings {
	/** Mirror task mutations into `.restask/tasks/<uid>.ics` (§15.3). */
	enableCacheMirror: boolean;
	/** Heading that starts the completed region; must equal the vault config `done_heading`. */
	doneHeading: string;
}

/** Defaults per §15.4. */
export const DEFAULT_SETTINGS: RestaskSettings = {
	enableCacheMirror: true,
	doneHeading: DEFAULT_DONE_HEADING,
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
		containerEl.createEl("h2", { text: "Restask" });

		new Setting(containerEl)
			.setName("Enable cache mirror")
			.setDesc("Mirror task mutations into .restask/tasks/<uid>.ics so the daemon can sync them.")
			.addToggle((toggle) =>
				toggle.setValue(this.plugin.settings.enableCacheMirror).onChange(async (value) => {
					this.plugin.settings.enableCacheMirror = value;
					await this.plugin.saveSettings();
				}),
			);

		new Setting(containerEl)
			.setName("Done heading")
			.setDesc("Heading that starts the completed region; must equal the vault config done_heading.")
			.addText((text) =>
				text.setValue(this.plugin.settings.doneHeading).onChange(async (value) => {
					this.plugin.settings.doneHeading = value;
					await this.plugin.saveSettings();
				}),
			);
	}
}
