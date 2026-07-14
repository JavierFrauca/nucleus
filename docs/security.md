# Seguridad

Modelo de amenazas y guía de **endurecimiento** para desplegar Nucleus en
producción. Esta página amplía lo esencial de
[operación → seguridad](operacion.md#seguridad); no lo sustituye.

## Modelo de seguridad

- **Sin acceso anónimo**: todo endpoint salvo `/healthz`, `/readyz` y `/metrics`
  requiere un **token** tipo API-key (`Authorization: Bearer nuc_…`).
- **Tokens con scopes por dominio**: cada token lleva una lista de `Scope`
  (`{ domain, perm }`); `perm` es `Read` < `Write` < `Admin`. La autorización se
  comprueba por operación y por dominio.
- **Cifrado en reposo siempre activo**: XChaCha20-Poly1305 para los valores,
  HMAC-SHA256 con clave para las claves de índice sensibles. No hay modo sin
  cifrar. Ver [cifrado en reposo](operacion.md#cifrado-en-reposo).
- **Rate limiting** por IP (token-bucket), opt-in.

## Endurecimiento de producción

### Transporte: TLS siempre

El servidor habla **HTTP plano**. NUNCA lo expongas directamente a una red no
confiable. Pon un reverse proxy (nginx/Caddy/Traefik) que termine TLS por
delante:

```
[cliente] --HTTPS--> [nginx/Caddy] --HTTP--> [nucleus-server :8080 (loopback)]
```

### Principio de mínimo privilegio con tokens

Da a cada consumidor el scope **mínimo**:

```json
// Una app de búsqueda web: solo lectura sobre un dominio
{ "name": "buscador-web", "scopes": [ { "domain": { "One": 1 }, "perm": "Read" } ] }
```

- **No reutilices** el token admin para aplicaciones.
- Usa `expires_at` (millis Unix) para caducar tokens de corta duración.
- **Rota** tokens con regularidad; revoca con `DELETE /v1/tokens/{id}`.
- El secreto plano se muestra **una sola vez** al crear; no se vuelve a poder
  leer (solo su hash SHA-256 se persiste).

### Rate limiting y proxies

`NUCLEUS_RATE_LIMIT_RPM` (token-bucket por IP) protege del abuso. Si Nucleus
corre tras un proxy:

- Por defecto limita por la **IP del peer** (la del proxy): todo el tráfico
  comparte un presupuesto. Eso puede ser lo que quieras (acotar el total).
- Si prefieres limitar por **cliente real**, activa `NUCLEUS_TRUST_PROXY=true`
  **solo si tu proxy sobrescribe/limpia `X-Forwarded-For`** entrante. Si confías
  en la cabecera sin filtrarla, un cliente puede falsificar IPs y saltarse su
  presupuesto. Ver [operación](operacion.md#seguridad).

### Claves de cifrado

| Modo | Cuándo | Notas |
|------|--------|-------|
| **Passphrase** (`NUCLEUS_PASSPHRASE` / `passphrase` en FFI) | Backups que salen de la máquina; portabilidad | Derivada con Argon2id. La misma frase reabre la BD en cualquier sitio. |
| **Clave de máquina** (defecto) | Despliegue en un solo nodo, cero config | Aleatoria, protegida por el SO (DPAPI en Windows, `0600` en Linux). Ligada a la máquina/usuario. |

**El fichero de clave vive separado de la BD**, nunca en su directorio ni en
los backups. Respáldalo **aparte**. Perder la clave = perder los datos.

Rotación: el FFI expone `nucleus_rekey` y el modo servidor tiene `rekey` (escribe
una copia re-cifrada con una clave nueva; la activas reabriendo sobre ella). La
rotación **resetea el índice de deduplicación** por hash (ver docs del engine).

### Aislamiento de red

- `NUCLEUS_ADDR` debe escuchar en **loopback** (`127.0Q.0.1:8080`) salvo que
  haya un proxy en otra máquina.
- Limita el acceso al puerto por firewall/security group.
- El token `Admin` puede crear dominios y tokens: trátalo como credencial
  privilegiada.

## Superficie expuesta

| Endpoint | Auth | Notas |
|----------|------|-------|
| `/healthz`, `/readyz` | sin | Solo estado; `/readyz` toca storage. |
| `/metrics` | sin | Texto Prometheus. **Protege por red/proxy** (expone contadores). |
| `/dashboard` | sin (es HTML inerte) | El propio dashboard se autentica en cada llamada `/v1/*` con el token que introduces. |
| `/v1/*` | **token + scope** | Todo el trabajo real. |

## Amenazas consideradas

- **Robo del fichero `.redb`**: sin la clave (passphrase o de máquina) los
  valores son ilegibles y las claves de índice están ofuscadas (HMAC). No es
  una protección total si el atacante también tiene la clave.
- **Fuga de token**: el scope limita el daño; revoca en cuanto lo detectes. No
  hay detección de anomalías built-in —mídelo vía logs del proxy.
- **Inyección en el `filter`**: el lenguaje de consulta se parsea con un parser
  propio (no es SQL); los términos se resuelven por índices, no por evaluación
  arbitraria. Aun así, valida siempre la entrada en tu aplicación.
- **Denegación de servicio**: rate limiting + load-shed de búsquedas + límite de
  tamaño de body (64 MB). Para más, protege en el borde (proxy/WAF).

## Reportar vulnerabilidades

**No abras un issue público.** Escribe al maintainer (vía GitHub, contacto en el
perfil) con: descripción del problema, impacto, y pasos/reporte de
reproducción. Agradecemos la divulgación responsable.
