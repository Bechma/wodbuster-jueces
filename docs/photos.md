# Fotos de puntuaciones

En `/competitions/{id}` hay dos opciones: **WOD único** espera un resultado y
**WOD múltiple** espera dos resultados. Cada opción tiene **📷 Tomar foto** y
**Galería o archivos**. Comparten la vista previa y la captura nativa del dispositivo.
La cámara en directo requiere HTTPS o localhost y permiso del navegador.

Cada scorecard conserva una sola imagen, categoría, equipo, juez y estado de incidencias.
La IA devuelve uno o dos resultados tipados por WOD. Si el número no coincide con el modo
elegido, falta un resultado, se repite un WOD o cualquier puntuación no es válida, no se
envía ninguno. Un bloqueo de cualquiera de sus WODs también impide enviar toda la scorecard.
Las fotos existentes siguen siendo de WOD único y conservan sus lecturas y confirmaciones.

La imagen se orienta y comprime en el dispositivo antes de subirla: máximo 1920 píxeles
en el lado mayor, WebP con calidad 0,82 (0,75 si supera 350 KB), con JPEG como alternativa.
No se amplían imágenes pequeñas ni se reduce agresivamente la resolución para alcanzar
un tamaño arbitrario. El máximo admitido es 1,5 MB; el servidor comprueba el formato real,
las dimensiones y que la imagen se pueda decodificar. La foto comprimida se guarda en
Postgres, sin duplicar el original de la cámara.

**Ver fotos** abre `/competitions/{id}/photos`. Muestra la foto, fecha, tamaño,
proveedor/modelo, estado, campos extraídos y posibles errores. El estado se actualiza
automáticamente. Las imágenes y resultados requieren la sesión de la cuenta propietaria.

**Juez detectado** muestra el nombre o identificador escrito en el apartado de juez de
la imagen. Es un dato informativo y de solo lectura: no se envía a Arena ni interviene
en la validación o el guardado de la puntuación. Si no es legible o no aparece, muestra
**No detectado**. Las fotos antiguas sin este dato se pueden volver a analizar con **↻**.
Corregir la puntuación conserva el juez detectado.

**Número de WODs** permite añadir el segundo resultado o quitarlo si se detectó por error.
Los dos resultados tienen WOD, formato, tiempo, repeticiones, peso y desempate propios.
Guardar valida el conjunto y encola ambos; quitar un resultado no borra la imagen ni
modifica por sí mismo las puntuaciones que ya existen en Arena.

Cada foto tiene un botón pequeño **↻** que vuelve a encolarla desde el análisis de IA,
con el modelo configurado actualmente; al obtener datos válidos se guarda otra vez en Arena.
En una scorecard múltiple se vuelven a analizar y enviar ambos resultados, incluidos
los que se hubieran guardado en un intento anterior.
Al pulsarlo aparece inmediatamente **Solicitando nuevo análisis…**, junto al encabezado
de la foto. El icono gira mientras se solicita o procesa el trabajo, y el aviso va mostrando
cola, análisis, preparación, guardado y resultado final. Si falla la solicitud o la consulta
de estado, el aviso queda visible. Perder la conexión para consultar no repite el envío.
Los campos aparecen como texto en una tabla, cada uno con un botón **Editar** que abre
su control. **Cerrar** oculta el control y conserva la corrección pendiente. Categoría,
equipo y WOD se seleccionan del contexto guardado; tiempo, repeticiones, kg y desempate
dependen del formato del WOD. Guardar permanece desactivado hasta que exista un cambio
real. Las filas modificadas se marcan en naranja; restaurar el valor original quita
la marca y, si no hay otros cambios, desactiva Guardar.
**Guardar cambios y encolar** valida y persiste la lectura corregida y el plan del formulario,
y encola directamente el envío a Arena, sin repetir el análisis de IA. También se pueden
corregir lecturas incompletas. Se conserva el modelo que produjo la lectura original.

Las dos acciones requieren CSRF y pertenencia a la cuenta/competición. Mientras una foto
está en cola, analizándose o enviándose, sus acciones permanecen bloqueadas. Una revisión
comparada atómicamente impide que dos peticiones o una pestaña antigua sobrescriban otra
edición. Las escrituras del worker también comprueban la revisión y el estado reclamados,
por lo que un trabajo antiguo no puede sobrescribir una corrección o un nuevo análisis.
La consulta automática de estados conserva los cambios sin guardar del formulario.

Las fotos y sus datos de procesamiento se conservan al borrar una competición local,
refrescar sus detalles o refrescar la lista, incluso si la competición ya no aparece
en Arena. La galería sigue limitada a la cuenta propietaria y a las competiciones
cargadas: al volver a cargar una competición con el mismo identificador, sus fotos
vuelven a estar disponibles. Borrar la información de una competición no borra fotos.
**Borrar foto**, en cada tarjeta, abre un modal con **Cancelar** y **Borrar foto
definitivamente**. Solo confirmar elimina la imagen y sus datos locales; la puntuación
guardada en Wodbuster se conserva. El borrado requiere CSRF, cuenta y revisión actual,
y permanece bloqueado mientras la foto se procesa o envía.

## Procesamiento

En la competición, **Ver fotos por categoría** muestra las fotos asignadas a esa
categoría. Cada WOD tiene **Ver fotos por WOD**, que mantiene la categoría seleccionada;
cada equipo tiene **Ver fotos por equipo**, que incluye sus fotos de todos los WODs
de esa categoría. Las vistas reutilizan la galería con sus estados y controles de edición.
El filtro se conserva al paginar y al consultar los estados en segundo plano.
El filtro de cualquiera de los dos WODs muestra la scorecard completa una sola vez.
Se usa la lectura actual de cada foto, incluidas las correcciones manuales: una foto
sin categoría asignada aparece en la galería general y en **Fotos sin categorizar o
con problemas**. Este nuevo botón también incluye categorías que ya no existen en
la competición, lecturas incompletas, errores de procesamiento o validación, envíos
sin confirmar, fallos al actualizar la copia local y avisos `needs_retake`, aunque
la puntuación se haya guardado. Incluye también duplicados y puntuaciones distintas
o no comparables con los datos de Arena. Conserva los controles de edición y refresco.
**Ver todas las fotos**
permite volver a ella. Si cambia la asignación durante la consulta, se muestra un aviso
para actualizar la lista y se conservan las ediciones pendientes.

**Comprobar sincronización**, junto a **Elegir foto**, consulta Arena en segundo plano
con Browserless y la cookie de la cuenta. Reutiliza el trabajo de carga de detalles;
al terminar se informa para actualizar la página. El resumen cuenta coincidencias,
diferencias, fotos duplicadas y fotos que no se pueden comparar. Cada tarjeta muestra
su comprobación y la fecha de los datos de Arena utilizados. La comparación utiliza
los valores estructurados del formato, incluidos tiempo, repeticiones, kg y desempate;
una puntuación ausente nunca equivale a cero. La comprobación consulta los resultados
y no guarda puntuaciones en Wodbuster.
En las scorecards múltiples se compara cada resultado por separado y se muestra su
comprobación junto a la confirmación de guardado. Una diferencia o un resultado no
comparable afecta a la scorecard completa. Los resúmenes cuentan imágenes, no WODs.

**Ver fotos no sincronizadas** abre `/competitions/{id}/photos?unsynced=true`.
Muestra las diferencias confirmadas con los últimos datos consultados de Arena,
utilizando la última foto de cada categoría, WOD y equipo. Las lecturas sin comprobar
o no comparables se consultan en la vista de problemas. El filtro se conserva en la
paginación y en la actualización automática de estados. Cuando una puntuación pasa
a coincidir con Arena, su foto sale de esta vista.

Si varias fotos tienen la misma cuenta, competición, categoría, WOD y equipo, todas
figuran como problemáticas. Se compara únicamente la última por `created_at` (con
`id` como desempate estable); editar una foto antigua no la convierte en la última.
Los duplicados se detectan entre todas las páginas. Borrar o reasignar una foto
recalcula el grupo y qué foto se utiliza, sin enviar puntuaciones a Arena.

El servidor inicia un worker con `photos::start`. Un advisory lock de Postgres, mantenido
en una conexión dedicada, permite un solo worker global por base de datos, incluso si
se inicia más de una instancia de la aplicación. Procesa una foto cada vez:

1. `queued` → `parsing`: obtiene el contexto de la competición de la cuenta propietaria.
2. Llama al mismo extractor de Rig/ChatGPT que el PoC, en `src/ai.rs`, con el proveedor nativo de Rig.
3. Guarda la lectura, la preparación y los errores de validación, conservando el proveedor
   y modelo elegidos al subir la imagen. Si faltan datos queda en `incomplete`.
4. `parsed` → `submitting`: vuelve a validar contra el contexto actual y reutiliza
   `ArenaBrowser::submit_score`. Espera si los navegadores de la aplicación están ocupados.
5. Abre **Edit** cuando corresponde y guarda los WODs de la scorecard en orden.
   Relee la vista de juez para verificar cada resultado y actualiza su puntuación local.
6. Persiste primero cada confirmación remota manteniendo `submitting` y las acciones
   bloqueadas. Solo termina en `succeeded` tras confirmar y copiar localmente todos
   los resultados; si falla una copia, termina en `saved_local_failed`.

Arena guarda cada WOD por separado. Si el primero se confirma y el segundo falla,
se conserva el primero y toda la scorecard queda con incidencia, indicando el WOD que
falló y cuántos resultados se confirmaron. Un fallo posterior a pulsar Guardar queda
en `unconfirmed`; nunca se reintenta automáticamente. Tras una interrupción, una
confirmación parcial también queda en `unconfirmed`, conservando las confirmaciones.
El usuario puede revisar Arena y reintentar explícitamente la scorecard completa.

Al cargar los detalles, los WODs ya guardados se abren con **Edit** para capturar sus
campos y el control real de **Guardar**. La exploración de formatos nunca guarda
puntuaciones y conserva los valores originales. Así, refrescar los detalles permite
seguir preparando correcciones y nuevos envíos de puntuaciones existentes. Los
snapshots anteriores que solo contienen **Edit** necesitan refrescarse una vez.

Si `needs_retake` es `true`, pero los campos son suficientes y válidos, continúa el envío.
La galería muestra un aviso cuando Arena confirma el guardado de esa lectura.

`failed` conserva el error de extracción o de un WOD cuyo envío no llegó a guardar;
si hubo otro resultado confirmado antes del fallo, también lo conserva.
`unconfirmed` indica que hay que comprobar Arena antes de repetir el envío.
`saved_local_failed` conserva la confirmación remota si falta actualizar la caché local.
Un reinicio reanuda el análisis interrumpido e invalida la revisión del worker anterior.
Un envío interrumpido se marca como `unconfirmed` si falta confirmar algún resultado,
o `saved_local_failed` si Arena ya confirmó todos; ninguno se repite automáticamente. Una petición de subida repetida con el
mismo UUID no duplica la foto ni el trabajo.

Se utiliza la sesión OAuth creada con `cargo run --example chatgpt_login`, guardada en
`$HOME/.config/wodbuster-jueces/auth.json` o en `CHATGPT_AUTH_FILE`, y `CHATGPT_MODEL`, con
`gpt-6-luna` por defecto. Consulta [autenticación de ChatGPT](chatgpt-auth.md) para preparar
otras ejecuciones o servicios. Postgres, Browserless y la cookie cifrada son los mismos
que usa la aplicación. El usuario debe haber cargado los detalles de la competición.

## Bloqueos de puntuaciones

En la competición, **Bloquear subidas** abre una pantalla independiente para bloquear
una categoría completa, un WOD en todas las categorías o una combinación categoría/WOD.
Los bloqueos pertenecen a la cuenta y competición actuales, se guardan en Postgres y
sobreviven a los refrescos de datos de Arena. Los WODs se agrupan por nombre
(ignorando mayúsculas y espacios sobrantes): «Giralda» aparece una sola vez y su
bloqueo con «Todas las categorías» cubre todos sus IDs, incluidas nuevas categorías
que aparezcan tras refrescar. Los bloqueos anteriores se convierten por nombre
cuando el WOD sigue disponible en el detalle guardado.

Tras la extracción de IA y antes de cada intento de envío se comprueban los bloqueos
vigentes. Una coincidencia deja la foto en `failed`, sin formulario preparado, con un
error explícito y `needs_retake=true`. Si `retake_reason` está ausente o vacío se rellena
con el motivo del bloqueo; si ya tenía contenido se conserva y el error de bloqueo se
guarda aparte. Esto también impide saltarse el bloqueo mediante correcciones manuales.
La foto y su lectura se conservan para revisión. Desbloquear no reenvía automáticamente
las fotos: hay que reintentarlas desde la galería.

La confirmación de un bloqueo espera a que terminen los envíos que ya estaban en curso.
Los reintentos por navegador ocupado liberan la exclusión y vuelven a leer las reglas.
La configuración y el rechazo no consultan ni modifican Arena.

## Pruebas

Las pruebas específicas usan lecturas simuladas, HTTP local y Postgres temporal:

```sh
cargo test --lib submission_blocks::
cargo test --test submission_blocks -- --ignored
cargo test --lib submission_blocks_reject -- --ignored
cargo test --lib storage::submission_blocks:: -- --ignored
```

### Suite existente

```sh
cargo test --all-targets
cargo test --lib photos::tests -- --include-ignored
cargo test --test photos_web --test photo_filters --test photo_sync --test photo_retention -- --ignored
cargo test --lib regression_tests -- --ignored
```

Las pruebas de integración requieren los servicios de `compose.dev.yml` y `DATABASE_URL`.
La cámara se simula en la prueba de Browserless, que comprueba la compresión y el cierre
de sus pistas. La prueba opcional siguiente llama a ChatGPT y **guarda realmente** la
puntuación de la imagen del PoC en Arena:

```sh
cargo test --test live_photos -- --ignored --nocapture
```

Referencias: [captura de imágenes](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Attributes/capture),
[compresión con canvas](https://developer.mozilla.org/en-US/docs/Web/API/HTMLCanvasElement/toBlob),
[advisory locks de Postgres](https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS).
