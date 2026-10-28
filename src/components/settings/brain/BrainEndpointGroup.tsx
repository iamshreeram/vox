import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../ui/Button";
import { Dropdown } from "../../ui/Dropdown";
import { Input } from "../../ui/Input";
import { SettingContainer } from "../../ui/SettingContainer";
import { ModelSelect } from "../PostProcessingSettingsApi/ModelSelect";
import { useSettings } from "../../../hooks/useSettings";
import { useBrainActions } from "./useBrainActions";
import {
  BRAIN_PRESETS,
  BRAIN_QUICK_FILLS,
  brainViewOf,
  canFetchModels,
  urlProblem,
  type BrainProviderId,
  type UrlProblem,
} from "./brainModel";

interface BrainEndpointGroupProps {
  /** The saved destination has been acknowledged for the connection test. */
  acked: boolean;
  /** Reports whether the provider, address and model drafts match the saved connection. */
  onDraftMatchChange: (matches: boolean) => void;
}

const URL_ERROR_KEY: Record<UrlProblem, string> = {
  empty: "settings.brain.endpoint.urlError.empty",
  invalid: "settings.brain.endpoint.urlError.invalid",
  scheme: "settings.brain.endpoint.urlError.scheme",
  userinfo: "settings.brain.endpoint.urlError.userinfo",
  plain_http_remote: "settings.brain.endpoint.urlError.plainHttp",
};

function isPresetId(value: string): value is BrainProviderId {
  return BRAIN_PRESETS.some((preset) => preset.id === value);
}

/**
 * Provider, endpoint address, token and model. Edits are kept as drafts and written
 * through the validated command only when the endpoint is a legal address.
 */
export const BrainEndpointGroup: React.FC<BrainEndpointGroupProps> = ({
  acked,
  onDraftMatchChange,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const actions = useBrainActions();

  const savedProvider = settings?.brain_provider_id ?? "";
  const savedUrl = settings?.brain_base_url ?? "";
  const savedModel = settings?.brain_model ?? "";
  const tokenSet = settings?.brain_token_set === true;

  const [providerDraft, setProviderDraft] = useState(savedProvider);
  const [urlDraft, setUrlDraft] = useState(savedUrl);
  const [modelDraft, setModelDraft] = useState(savedModel);
  // Test and Fetch models only ever target the address shown in the form.
  const draftMatches =
    providerDraft === savedProvider &&
    urlDraft.trim() === savedUrl &&
    modelDraft.trim() === savedModel;
  const canFetch =
    acked &&
    draftMatches &&
    settings !== null &&
    canFetchModels(brainViewOf(settings));
  const [urlError, setUrlError] = useState<UrlProblem | null>(null);
  // The token lives in state only while it is being typed; it is cleared on blur.
  const [tokenDraft, setTokenDraft] = useState("");
  const [fetchedModels, setFetchedModels] = useState<string[]>([]);
  const [fetching, setFetching] = useState(false);

  useEffect(() => {
    setProviderDraft(savedProvider);
  }, [savedProvider]);
  useEffect(() => {
    setUrlDraft(savedUrl);
  }, [savedUrl]);
  useEffect(() => {
    setModelDraft(savedModel);
  }, [savedModel]);
  useEffect(() => {
    onDraftMatchChange(draftMatches);
  }, [draftMatches, onDraftMatchChange]);

  const providerOptions = BRAIN_PRESETS.map((preset) => ({
    value: preset.id,
    label: t(`settings.brain.providers.${preset.id}`),
  }));

  const modelOptions = [
    ...fetchedModels,
    ...(modelDraft !== "" && !fetchedModels.includes(modelDraft)
      ? [modelDraft]
      : []),
  ].map((model) => ({ value: model, label: model }));

  // Writes the connection only when it is legal and actually changed.
  const save = async (
    provider: string,
    url: string,
    model: string,
  ): Promise<void> => {
    if (!isPresetId(provider) || urlProblem(url) !== null) return;
    const nextUrl = url.trim();
    const nextModel = model.trim();
    if (
      provider === savedProvider &&
      nextUrl === savedUrl &&
      nextModel === savedModel
    ) {
      return;
    }
    await actions.saveConnection(provider, nextUrl, nextModel);
  };

  const handleProvider = (value: string) => {
    if (!isPresetId(value)) return;
    const preset = BRAIN_PRESETS.find((entry) => entry.id === value);
    // Custom keeps the address that is already there; a service fills its own.
    const nextUrl =
      value === "custom" || preset === undefined ? urlDraft : preset.baseUrl;
    setProviderDraft(value);
    setUrlDraft(nextUrl);
    setUrlError(null);
    void save(value, nextUrl, modelDraft);
  };

  const handleQuickFill = (url: string) => {
    setProviderDraft("custom");
    setUrlDraft(url);
    setUrlError(null);
    void save("custom", url, modelDraft);
  };

  const handleUrlBlur = () => {
    const problem = urlProblem(urlDraft);
    setUrlError(problem);
    if (problem === null) void save(providerDraft, urlDraft, modelDraft);
  };

  const handleModel = (value: string) => {
    setModelDraft(value);
    void save(providerDraft, urlDraft, value);
  };

  const handleFetch = async () => {
    setFetching(true);
    const models = await actions.fetchModels();
    setFetching(false);
    if (models !== null) setFetchedModels(models);
  };

  const handleTokenBlur = () => {
    const token = tokenDraft.trim();
    if (token === "") return;
    setTokenDraft("");
    void actions.saveToken(token);
  };

  const handleClearToken = () => {
    setTokenDraft("");
    void actions.saveToken("");
  };

  return (
    <div className="px-4 py-3 space-y-1">
      <SettingContainer
        title={t("settings.brain.endpoint.provider.title")}
        description={t("settings.brain.endpoint.provider.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div data-testid="brain-provider" className="flex items-center gap-2">
          <Dropdown
            options={providerOptions}
            selectedValue={providerDraft === "" ? null : providerDraft}
            onSelect={handleProvider}
            className="min-w-[280px]"
          />
        </div>
      </SettingContainer>

      <SettingContainer
        title={t("settings.brain.endpoint.quickFill.title")}
        description={t("settings.brain.endpoint.quickFill.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex items-center gap-2">
          {BRAIN_QUICK_FILLS.map((fill) => (
            <Button
              key={fill.id}
              data-testid={`brain-quickfill-${fill.id}`}
              variant="secondary"
              size="sm"
              onClick={() => handleQuickFill(fill.baseUrl)}
            >
              {t(`settings.brain.endpoint.quickFill.${fill.id}`)}
            </Button>
          ))}
        </div>
      </SettingContainer>

      <SettingContainer
        title={t("settings.brain.endpoint.baseUrl.title")}
        description={t("settings.brain.endpoint.baseUrl.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex flex-col gap-1 min-w-[320px]">
          <Input
            data-testid="brain-base-url"
            aria-label={t("settings.brain.endpoint.baseUrl.title")}
            aria-invalid={urlError !== null}
            aria-describedby={urlError !== null ? "brain-url-error" : undefined}
            type="text"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            value={urlDraft}
            placeholder={t("settings.brain.endpoint.baseUrl.placeholder")}
            onChange={(event) => setUrlDraft(event.target.value)}
            onBlur={handleUrlBlur}
            className="w-full"
          />
          {urlError !== null && (
            <p
              id="brain-url-error"
              data-testid="brain-url-error"
              role="alert"
              className="text-xs text-red-500"
            >
              {t(URL_ERROR_KEY[urlError])}
            </p>
          )}
        </div>
      </SettingContainer>

      <SettingContainer
        title={t("settings.brain.token.title")}
        description={t("settings.brain.token.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex flex-col gap-1 min-w-[320px]">
          <div className="flex items-center gap-2">
            <Input
              data-testid="brain-token"
              aria-label={t("settings.brain.token.title")}
              type="password"
              autoComplete="new-password"
              spellCheck={false}
              value={tokenDraft}
              placeholder={t("settings.brain.token.placeholder")}
              onChange={(event) => setTokenDraft(event.target.value)}
              onBlur={handleTokenBlur}
              className="flex-1"
            />
            <Button
              variant="secondary"
              size="sm"
              disabled={!tokenSet}
              onClick={handleClearToken}
            >
              {t("settings.brain.token.clear")}
            </Button>
          </div>
          <p data-testid="brain-token-status" className="text-xs text-mid-gray">
            {tokenSet
              ? t("settings.brain.token.saved")
              : t("settings.brain.token.none")}
          </p>
        </div>
      </SettingContainer>

      <SettingContainer
        title={t("settings.brain.model.title")}
        description={t("settings.brain.model.description")}
        descriptionMode="tooltip"
        layout="stacked"
        grouped={true}
      >
        <div className="flex items-center gap-2">
          <div data-testid="brain-model" className="flex-1 min-w-[320px]">
            <ModelSelect
              value={modelDraft}
              options={modelOptions}
              isLoading={fetching}
              placeholder={t("settings.brain.model.placeholder")}
              onSelect={handleModel}
              onCreate={handleModel}
              onBlur={() => undefined}
              className="flex-1"
            />
          </div>
          <Button
            data-testid="brain-fetch-models"
            variant="secondary"
            size="sm"
            disabled={!canFetch || fetching}
            onClick={() => void handleFetch()}
          >
            {t("settings.brain.model.fetch")}
          </Button>
        </div>
      </SettingContainer>
    </div>
  );
};
