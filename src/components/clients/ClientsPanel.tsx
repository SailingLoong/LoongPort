import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  clientsApi,
  type ClientStatus,
  type ClientCredential,
  type ClientConnection,
  type ClientPreview,
} from "@/lib/api/clients";

export function ClientsPanel() {
  const { t } = useTranslation();
  const [clients, setClients] = useState<ClientStatus[]>([]);
  const [credentials, setCredentials] = useState<ClientCredential[]>([]);
  const [error, setError] = useState("");
  const refresh = async () => {
    const [next, keys] = await Promise.all([
      clientsApi.list(),
      clientsApi.credentials(),
    ]);
    setClients(next);
    setCredentials(keys);
  };
  useEffect(() => {
    void refresh().catch(() => setError(t("clients.operationFailed")));
  }, [t]);
  return (
    <div className="page-content space-y-4">
      <p className="text-sm text-muted-foreground">
        {t("clients.description")}
      </p>
      {error && <p role="alert">{error}</p>}
      {clients.map((client) => (
        <ClientCard
          key={client.descriptor.id}
          client={client}
          credentials={credentials}
          refresh={refresh}
        />
      ))}
    </div>
  );
}
function ClientCard({
  client,
  credentials,
  refresh,
}: {
  client: ClientStatus;
  credentials: ClientCredential[];
  refresh: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [route, setRoute] = useState(client.descriptor.routeApps[0] ?? "");
  const [models, setModels] = useState("");
  const [credential, setCredential] = useState("");
  const [preview, setPreview] = useState<{
    request: ClientConnection;
    value: ClientPreview;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [applied, setApplied] = useState(false);
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch {
      setError(t("clients.operationFailed"));
      setPreview(null);
    } finally {
      setBusy(false);
    }
  };
  const choices = credentials.filter(
    (key) =>
      key.clientId === client.descriptor.id &&
      key.routeApp === route &&
      !key.revoked,
  );
  const modelIds = () => [
    ...new Set(
      models
        .split(/[\n,]/)
        .map((value) => value.trim())
        .filter(Boolean),
    ),
  ];
  return (
    <section className="space-y-3 rounded-xl border border-border-default bg-card p-5">
      <h2 className="font-semibold">{client.descriptor.name}</h2>
      <p className="text-sm text-muted-foreground">
        {t(`clients.${client.status}`)}
      </p>
      {client.connection && (
        <p className="text-sm">
          {t("clients.sharedRoute", { route: client.connection.routeApp })}
        </p>
      )}
      {client.recoveryRequired ? (
        <Button
          disabled={busy}
          onClick={() =>
            void run(async () => {
              await clientsApi.recover(client.descriptor.id);
              await refresh();
            })
          }
        >
          {t("clients.recover")}
        </Button>
      ) : (
        <>
          <label className="block text-sm">
            {t("clients.route")}
            <select
              className="ml-2 rounded border bg-background p-2"
              disabled={busy}
              value={route}
              onChange={(event) => {
                setRoute(event.target.value);
                setCredential("");
                setPreview(null);
              }}
            >
              {client.descriptor.routeApps.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>
          </label>
          <label className="block text-sm">
            {t("clients.models")}
            <Input
              disabled={busy}
              value={models}
              onChange={(event) => {
                setModels(event.target.value);
                setPreview(null);
              }}
            />
          </label>
          <label className="block text-sm">
            {t("clients.credential")}
            <select
              className="ml-2 rounded border bg-background p-2"
              disabled={busy}
              value={credential}
              onChange={(event) => {
                setCredential(event.target.value);
                setPreview(null);
              }}
            >
              <option value="">{t("clients.chooseCredential")}</option>
              {choices.map((key) => (
                <option key={key.id} value={key.id}>
                  {key.allowedModels.join(", ")}
                </option>
              ))}
            </select>
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              disabled={busy || !models.trim()}
              onClick={() =>
                void run(async () => {
                  const created = await clientsApi.createCredential(
                    client.descriptor.id,
                    route,
                    modelIds(),
                  );
                  await refresh();
                  setCredential(created.id);
                  setPreview(null);
                })
              }
            >
              {t("clients.createCredential")}
            </Button>
            <Button
              disabled={busy || !credential || !models.trim()}
              onClick={() =>
                void run(async () => {
                  const request = {
                    clientId: client.descriptor.id,
                    credentialRef: credential,
                    routeApp: route,
                    models: modelIds(),
                  };
                  setPreview({
                    request,
                    value: await clientsApi.preview(request),
                  });
                })
              }
            >
              {t("clients.preview")}
            </Button>
            <Button
              variant="outline"
              disabled={busy || !credential}
              onClick={() =>
                void run(async () => {
                  await clientsApi.revoke(client.descriptor.id, credential);
                  setCredential("");
                  setPreview(null);
                  await refresh();
                })
              }
            >
              {t("clients.revoke")}
            </Button>
            {client.connection && (
              <Button
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await clientsApi.disconnect(client.descriptor.id);
                    setApplied(false);
                    setPreview(null);
                    await refresh();
                  })
                }
              >
                {t("clients.disconnect")}
              </Button>
            )}
          </div>
          {preview && (
            <div className="space-y-2 rounded border p-3">
              <p>
                {t("clients.previewDescription", {
                  count: preview.value.modelCount,
                  route: preview.request.routeApp,
                })}
              </p>
              <p className="text-sm">{preview.request.models.join(", ")}</p>
              <Button
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await clientsApi.apply(
                      preview.request,
                      preview.value.token,
                    );
                    setApplied(true);
                    setPreview(null);
                    await refresh();
                  })
                }
              >
                {t("clients.apply")}
              </Button>
            </div>
          )}
        </>
      )}
      {applied && (
        <p role="status" className="text-sm">
          {t("clients.applied")}
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}
