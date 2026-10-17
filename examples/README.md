# PoC de lectura de puntuaciones

El ejemplo procesa una imagen con `Extractor<Scorecard>` de Rig y su proveedor nativo ChatGPT. Una scorecard contiene uno o dos resultados del mismo equipo, categoría y juez. La lectura usa el mismo `ScoreValue` que la aplicación y el formato concreto de cada WOD y categoría.

```sh
cargo run --example poc_agent -- foto.png contexto.json > resultado.json
```

Antes de la primera extracción, ejecuta `cargo run --example chatgpt_login` y completa el flujo OAuth de dispositivo. Rig guarda la sesión en `$HOME/.config/wodbuster-jueces/auth.json` y la renueva automáticamente. `CHATGPT_AUTH_FILE` permite elegir otro archivo tanto para el login como para la aplicación. `CHATGPT_MODEL` es opcional; el modelo predeterminado es `gpt-6-luna` (Luna 6). Consulta [cómo autenticar futuras ejecuciones](../docs/chatgpt-auth.md). Se admiten imágenes PNG, JPEG y WebP, identificadas por su contenido. `--validate` funciona sin autenticación de IA ni llamadas al modelo.

La extracción utiliza un esquema JSON estricto y un límite total de 120 segundos. Un fallo o un timeout de ChatGPT detiene el proceso antes de enviar una puntuación a Arena.

`contexto.json` debe contener un `CompetitionDetail` real de la cuenta correspondiente, obtenido con `SessionStore::competition_detail` o de `competition_details.snapshot`. Carga/refresca previamente los detalles de esa competición para disponer de los campos condicionales y las referencias del formulario. No se usan identificadores de ejemplo como sustituto de esos datos.

El proveedor recibe la imagen, los nombres e identificadores de competición/categorías/WODs/equipos y la semántica de las puntuaciones. No recibe cookies, miembros de equipos, puntuaciones anteriores, URLs ni controles del formulario.

Los formatos se deduplican en un catálogo compartido (`formats`); las filas de WODs lo referencian por índice. Las columnas de las filas están descritas en `columns`. Así, categorías que comparten un formato no repiten el esquema completo en la petición.

## Resultado

La salida estándar es exclusivamente JSON. Los diagnósticos y el uso de tokens se escriben en stderr.

- `reading`: `category_id`, `wod_id`, `team_reference` y `score_value` tipado. `needs_retake` y `retake_reason` son observaciones opcionales que no bloquean la preparación.
- `ready_for_form`: indica si la lectura y las referencias permiten preparar el formulario.
- `prepared`: datos resueltos contra el contexto, con competición, categoría, WOD, equipo, inscripción, `gid`, puntuación y plan de formulario. Es `null` si falta información o no es compatible.
- `validation_errors`: motivo que impide preparar el formulario. Un problema de contexto requiere refrescar sus datos; una imagen dudosa requiere otra foto.

Para dos WODs, `reading` contiene los datos comunes y `results`, una lista de
`{wod_id, score_value}`; `prepared` es una lista de dos planes. Ambos deben ser
válidos y corresponder a WODs distintos para habilitar el envío. Un resultado
ilegible impide enviar el conjunto. `--submit` envía los WODs en orden y devuelve
`submissions` con las confirmaciones individuales; si falla el segundo, conserva
en la salida la confirmación del primero. Para un WOD se conserva también el
contrato anterior de `reading`, `prepared` y `submission`, y `--validate` acepta
lecturas antiguas y nuevas.

El valor utiliza uno de estos formatos:

```json
{"kind":"completed","time":{"minutes":7,"seconds":20},"tie_break":null}
{"kind":"not_completed","repetitions":2,"tie_break":null}
{"kind":"weight","kilograms":82.5}
{"kind":"repetitions","repetitions":52}
{"kind":"time","time":{"minutes":7,"seconds":20}}
```

Si están presentes los cuatro datos necesarios y son válidos contra el contexto, se genera `prepared` y `ready_for_form` es `true`, incluso si el modelo devuelve `needs_retake: true`. No se requieren confianza, transcripción ni notas. Con `--submit`, una preparación válida permite continuar con el envío.

Un resultado ilegible es `null`; no se transforma en cero. Un cero visible sí es válido. Se rechazan datos ausentes, tiempos con segundos fuera de 00–59, pesos negativos, formatos incompatibles y referencias que no pertenezcan a la categoría elegida. La corrección de la lectura visual sigue dependiendo del modelo.

## Plan de rellenado

La IA solo devuelve identificadores y puntuaciones. `scorecard::prepare_score` resuelve localmente la URL, los controles y las opciones exactas desde el contexto guardado.

`prepared.form.steps` se ejecutará en orden. Para un WOD no completado, primero selecciona el valor real de «No» y marca `wait_for_postback: true`; después rellena las repeticiones. Tras ese postback el ejecutor deberá volver a localizar los controles. Para peso, selecciona la opción real de kg antes de introducir la cantidad. El desempate opcional se vacía si no forma parte de la nueva lectura, evitando conservar uno anterior.

El plan incluye `save_control`. Sin `--submit`, el PoC solo genera el plan. El JSON no contiene credenciales ni ViewState.

## Enviar una puntuación a Wodbuster

`--submit` ejecuta el plan y **guarda realmente la puntuación**, sustituyendo la anterior de ese equipo y WOD:

```sh
cargo run --example poc_agent -- --submit examples/poc_agent.png tmp/poc-context.json > tmp/enviado.json
```

Requiere los servicios de `compose.dev.yml`, las variables `DATABASE_URL`, `SESSION_SECRET`, `BROWSER_URL` y `BROWSER_TOKEN` de la aplicación y una cookie `.WBAuth` válida guardada en Postgres tras acceder a la aplicación. La competición debe tener sus detalles cargados para esa cuenta. No lee la contraseña de `tmp/SECRET.json` ni inicia sesión automáticamente.

Para enviar, el contexto del fichero identifica la competición; el PoC usa el snapshot actual de esa cuenta en Postgres para extraer y validar la lectura. Reutiliza la configuración de Browserless de la aplicación, incluido `stealth`. Si hay varias cuentas con esa competición, hay que elegir una sesión con `--session <UUID>` (el `id` de `wodbuster_sessions`); nunca selecciona una cuenta distinta de forma arbitraria.

El ejecutor comprueba la URL, el WOD, el orden actual de los WODs, los controles y sus opciones. Si ya hay una puntuación, abre primero «Edit» del WOD seleccionado. Espera los postbacks al cambiar «Completado» y vuelve a localizar los campos. Pulsa únicamente el botón Guardar de ese WOD. Después recarga la página, lee el resultado persistido de la vista de juez y reabre «Edit» para comprobar modo, puntuación y desempate. No vuelve a guardar durante esa comprobación. Finalmente actualiza solo esa puntuación en Postgres.

El API público del equipo puede seguir devolviendo `Score: null` aunque la vista autenticada del juez ya muestre el resultado. La confirmación utiliza el resultado de juez y los campos releídos. `metadata.api` conserva la respuesta pública original y `metadata.judge_result` conserva el resultado confirmado.

La salida añade `submission` con `status: "saved"`, `verified: true`, `local_updated: true` y la puntuación releída cuando todo termina correctamente. Una lectura inválida o un fallo anterior a pulsar Guardar produce `status: "not_submitted"` y un código de salida distinto de cero: no se ha guardado en Arena. Un fallo desde el intento de Guardar produce `status: "unconfirmed"`: **comprueba Arena antes de repetir el envío**, porque el servidor podría haberlo guardado. No hay reintentos automáticos de Guardar. Si Arena confirma el cambio pero falla Postgres, se devuelve `status: "saved"` con `local_updated: false` y código de salida distinto de cero; refresca la competición desde la aplicación.

El rellenado espera las tareas de la máscara de entrada antes de salir del campo, para dejar el desempate opcional realmente vacío (`""`). Antes de Guardar se ejecuta el validador de Arena para el grupo del WOD seleccionado; si rechaza algún campo, se devuelve su motivo sin pulsar Guardar ni esperar un postback. Un `00:00` real se conserva como valor y no se confunde con un campo vacío.

## Validación sin llamadas de IA

Se puede validar una lectura o reevaluar la salida completa de una ejecución previa contra otro snapshot:

```sh
cargo run --example poc_agent -- --validate resultado.json contexto.json > validado.json
cargo test --test scorecard
```

`--validate` también admite salidas antiguas: ignora los campos eliminados `is_confident`, `score_raw` y `score_notes`. Estos campos no aparecen en el nuevo esquema ni en la salida.

También puedes enviar una lectura ya obtenida, sin gastar otra llamada a la IA:

```sh
cargo run --example poc_agent -- --validate --submit tmp/poc-result.json tmp/poc-context.json > tmp/enviado.json
```

El contrato y la preparación son reutilizables desde `src/scorecard.rs`. El PoC y el worker comparten la extracción de `src/ai.rs` y el cliente nativo de ChatGPT.

Referencias: [extractores de Rig](https://rig.rs/docs/concepts/extractors/), [proveedor ChatGPT de Rig](https://docs.rs/rig/latest/rig/providers/chatgpt/index.html), [GPT-6 Luna](https://developers.openai.com/api/docs/models/gpt-6-luna), [autenticación OAuth de ChatGPT](../docs/chatgpt-auth.md).
