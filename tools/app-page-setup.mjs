const SETUP_FIELDS = new Set(["steps"]);
const STEP_FIELDS = new Set(["text", "link", "command"]);
const LINK_FIELDS = new Set(["label", "url"]);

function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value;
}

function exactFields(value, allowed, label) {
  for (const field of Object.keys(value)) {
    if (!allowed.has(field)) throw new Error(`unknown field '${field}' in ${label}`);
  }
}

function boundedText(value, label, maximum) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  if (value.length > maximum) throw new Error(`${label} must be at most ${maximum} characters`);
  return value;
}

function validatedLink(value, label) {
  const link = object(value, label);
  exactFields(link, LINK_FIELDS, label);
  const linkLabel = boundedText(link.label, `${label} label`, 80);
  const urlText = boundedText(link.url, `${label} URL`, 500);
  let url;
  try {
    url = new URL(urlText);
  } catch {
    throw new Error(`${label} URL must be an absolute HTTPS URL`);
  }
  if (url.protocol !== "https:" || url.username || url.password) {
    throw new Error(`${label} URL must be an absolute HTTPS URL without credentials`);
  }
  return { label: linkLabel, url: urlText };
}

export function validatedSetup(app) {
  if (app.setup === undefined) return null;
  const label = `${app.id || "app"} setup`;
  const setup = object(app.setup, label);
  exactFields(setup, SETUP_FIELDS, label);
  if (!Array.isArray(setup.steps) || setup.steps.length === 0 || setup.steps.length > 6) {
    throw new Error(`${label} steps must contain between 1 and 6 entries`);
  }
  const steps = setup.steps.map((value, index) => {
    const stepLabel = `${label} step ${index + 1}`;
    const step = object(value, stepLabel);
    exactFields(step, STEP_FIELDS, stepLabel);
    return {
      text: boundedText(step.text, `${stepLabel} text`, 240),
      ...(step.link === undefined ? {} : { link: validatedLink(step.link, `${stepLabel} link`) }),
      ...(step.command === undefined
        ? {}
        : { command: boundedText(step.command, `${stepLabel} command`, 200) })
    };
  });
  return { steps };
}

export function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

export function setupPanel(app) {
  const setup = validatedSetup(app);
  if (!setup) return "";
  const steps = setup.steps
    .map(step => {
      const link = step.link
        ? ` <a href="${escapeHtml(step.link.url)}">${escapeHtml(step.link.label)}</a>`
        : "";
      const command = step.command
        ? `\n        <code class="setup-command">${escapeHtml(step.command)}</code>`
        : "";
      return `      <li>${escapeHtml(step.text)}${link}${command}</li>`;
    })
    .join("\n");
  return `
  <section class="panel prerequisites" aria-labelledby="prerequisites-title">
    <p class="eyebrow">App setup</p>
    <h2 id="prerequisites-title">Before you install</h2>
    <ol>
${steps}
    </ol>
  </section>`;
}
