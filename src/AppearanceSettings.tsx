import { themes, type Theme } from "./model";

interface AppearanceSettingsProps {
  selected: Theme;
  disabled: boolean;
  onChange: (theme: Theme) => void;
}

export function AppearanceSettings({ selected, disabled, onChange }: AppearanceSettingsProps) {
  function renderChoices(additional: boolean) {
    return (
      <fieldset
        className="theme-grid"
        aria-label={additional ? "Additional themes" : "Default themes"}
        disabled={disabled}
      >
        {themes.filter((theme) => theme.additional === additional).map((theme) => (
          <label className="theme-option" key={theme.id}>
            <span className="theme-sample" data-theme={theme.id} aria-hidden="true">
              <span /><i /><b />
            </span>
            <span className="theme-label">
              <input
                type="radio"
                name="theme"
                checked={theme.id === selected}
                onChange={() => onChange(theme.id)}
              />
              <strong>{theme.name}</strong>
            </span>
            <small>{theme.description}</small>
          </label>
        ))}
      </fieldset>
    );
  }

  const selectedAdditional = themes.some((theme) => theme.additional && theme.id === selected);
  return (
    <section className="panel panel-body" aria-labelledby="appearance-heading">
      <h2 id="appearance-heading">Appearance</h2>
      {renderChoices(false)}
      <details className="theme-more" open={selectedAdditional || undefined}>
        <summary>More themes</summary>
        {renderChoices(true)}
      </details>
    </section>
  );
}
