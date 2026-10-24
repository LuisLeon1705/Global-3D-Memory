# Global 3D Memory (`cgm`)

Es una memoria global y persistente para un agente de IA de programación. Es **suya**, la misma en
todas sus sesiones y en todos tus proyectos: quién es contigo, qué le enseñaste, qué decidieron y
por qué, qué dejó pendiente y lo que aprendió de cada proyecto.

Se construye sobre el motor [`3D-Memory` (`cthg-core`)](https://github.com/LuisLeon1705/3D_Memory).
Cada recuerdo vive en tres ejes:

- **X, el lugar:** a qué carpeta o tema aplica. Pierde fuerza con la distancia en el árbol (λ^d).
- **Y, el tiempo:** cuándo se aprendió. Es monótono.
- **χ, la certeza:** de −1 a +1, calculada a partir de la evidencia Beta(α, β) de cada origen.

Una memoria nueva se apila sobre las anteriores sin borrar nada: se refuerza, se sustituye o se
contradice.

## Por qué así

Una memoria de agente suele fallar de dos formas:

- **Cuesta en cada mensaje.** Por ejemplo, llama a un modelo para resumir o para decidir qué recordar.
- **Se pierde entre proyectos.** Una nota por repo nunca llega al siguiente.

`cgm` separa la lectura de la escritura:

| Momento | Quién lo hace | Coste |
|---|---|---|
| Al iniciar la sesión | `cgm hook session-start` (o primer turno de `cgm hook antigravity`) | ningún modelo |
| En cada mensaje del usuario | `cgm hook prompt` (o siguientes turnos de `cgm hook antigravity`) | ningún modelo |
| Al cerrar un momento que vale la pena | el agente lanza **un** subagente pequeño (skill `memoria-global`) | una llamada |

**Al iniciar la sesión** se inyecta lo siguiente:

- El **núcleo**: las lecciones de identidad.
- Lo que aplica a la carpeta de trabajo y a las carpetas que la contienen.
- Los planes pendientes del agente allí.

**Con cada mensaje** solo llega lo que esas palabras evocan y la sesión aún no vio. Si no hay nada,
no se escribe nada. Los presupuestos de texto son pequeños y fijos, así que la memoria nunca llena la
conversación.

**La escritura nunca es automática.** No hay hooks que lancen modelos, ni guardados programados, ni
bucles. El agente decide cuándo merece la pena guardar y delega en un subagente barato, que hace esto:

1. Lee lo nuevo de la conversación con `cgm sesion --id <ID>`. Recibe solo el diálogo: sin
   herramientas, sin recordatorios del sistema y empezando donde terminó el último guardado.
2. Decide como mucho 8 entradas.
3. Las guarda con `cgm anotar --sesion <ID> archivo.json`.

## Lugares (eje X)

| `donde` | Ruta interna | Para |
|---|---|---|
| `yo` | `""` (raíz) | lecciones sobre el agente mismo; con `nucleo: true`, presentes en toda sesión |
| `persona:<Nombre>` | `@personas/<Nombre>` | quién es alguien y qué prefiere |
| `oficio:<tema>` | `@oficio/<tema>` | saber técnico que sirve en cualquier proyecto |
| `proyecto` / `carpeta` / una ruta | carpeta relativa a tu carpeta de usuario | lo de un proyecto concreto |

Lo aprendido en `~/Projects/Juegos` llega a cada proyecto que haya debajo, cada vez más tenue con la
profundidad. No llega a `~/Projects/Finanzas`.

## Certeza (χ)

- Lo que concluye el agente (`origen: agente`) nace con evidencia neutra, χ ≈ 0.
- Lo que dice el usuario (`origen: usuario`) nace con más.
- Repetir un recuerdo lo refuerza en vez de duplicarlo.
- `cgm confirmar <id>` y `cgm negar <id>` mueven la evidencia.
- Un plan (`tipo: plan`) espera con χ −1 hasta que algo lo da por cumplido.
- Un recuerdo sustituido (`sustituye`) deja de inyectarse en cuanto su sustituto tiene χ > 0, pero
  sigue en la memoria. Si el sustituto lo concluyó el agente (χ ≈ 0), el viejo sigue vigente hasta
  que el usuario confirme el nuevo (`cgm confirmar <id>`): una conclusión del agente no tapa por sí
  sola lo que dijo el usuario.

## Una memoria por agente (`CGM_AGENT`)

Cada agente tiene su propia carpeta en `~/.cthg/agentes/<slug(CGM_AGENT)>/`, separada de la de
cualquier otro agente que use la misma máquina.

- **`CGM_AGENT` es obligatoria** (salvo que definas `CGM_DIR` explícitamente).
- Sin `CGM_AGENT` ni `CGM_DIR`, no se escribe nada: `anotar`, `hecho`, `confirmar`, `negar` y
  `nucleo` devuelven error indicando cómo configurarlo, y los hooks salen con código `0` en silencio
  sin inyectar nada.

## Instalación

Requisitos: Rust estable y el motor clonado al lado, en `../3D-Memory`.

```
cargo install --path . --locked
```

### En Claude Code

1. **Hooks.** En `~/.claude/settings.json`:

   ```json
   {
     "env": { "CGM_AGENT": "<nombre del agente>", "CGM_USER": "<tu nombre>" },
     "hooks": {
       "SessionStart":     [{ "hooks": [{ "type": "command", "command": "cgm hook session-start", "timeout": 10 }] }],
       "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "cgm hook prompt", "timeout": 10 }] }]
     }
   }
   ```

2. **Skill.** Copia `skill/claude-code/SKILL.md` a `~/.claude/skills/memoria-global/SKILL.md`.

### En Antigravity

1. **Hooks.** En `~/.gemini/config/hooks.json`:

   ```json
   {
     "cgm-memoria": {
       "PreInvocation": [
         {
           "type": "command",
           "command": "cgm --agente <nombre del agente> --usuario <tu nombre> hook antigravity",
           "timeout": 10
         }
       ]
     }
   }
   ```

2. **Skill.** Copia `skill/antigravity/SKILL.md` a `~/.gemini/config/skills/memoria-global/SKILL.md`.

### Variables

| Variable | Para qué | Por defecto |
|---|---|---|
| `CGM_AGENT` | nombre del agente (ubicación, cabecera inyectada y registro) | obligatoria (sin valor no se escribe ni inyecta) |
| `CGM_DIR` | carpeta explícita de la memoria | `~/.cthg/agentes/<slug(CGM_AGENT)>/` |
| `CGM_USER` | cómo se llama al usuario en lo que lee el subagente | `Usuario` |
| `CTHG_AI_REGISTRY` | registro de memorias de IA | `~/.cthg/ai-memories.json` |

## Verla

La memoria se registra sola en `~/.cthg/ai-memories.json`. El visor
[`memviz`](https://github.com/LuisLeon1705/3D-Memory-Visualizer) la muestra en su sección
**AI Memory** como una constelación 3D:

- El centro es el núcleo del agente.
- Los astros son las personas, los oficios y las carpetas.
- La altura es el tiempo.
- La luz es χ.

Si tienes varios agentes, cada uno con su propia memoria, todos aparecen ahí, cada uno por separado.

## Comandos

```
cgm hook session-start|prompt|antigravity          los hooks (leen el JSON del hook por stdin)
cgm sesion --id <ID> [--todo] [--formato claude|antigravity]   lo dicho desde lo último guardado
cgm anotar [--sesion <ID>] [--cwd <DIR>] [ARCHIVO|-]
cgm recordar "<texto>" [--cwd <DIR>]
cgm lista | agenda | donde [DIR] | ruta
cgm hecho | confirmar | negar <id>
cgm nucleo <id> [--quitar]
```

Formato de `anotar`:

```json
[{"texto": "…", "tipo": "regla|decision|hecho|incidente|convencion|plan|comando|sesion",
  "donde": "yo|proyecto|carpeta|persona:<Nombre>|oficio:<tema>|<ruta>",
  "origen": "usuario|agente", "causa": "…", "sustituye": 12, "contradice": 4,
  "nucleo": false, "niega": false}]
```

## Archivos

- `~/.cthg/agentes/<agente>/memoria.cthg`: la memoria, en el binario CTHG v3 con CRC.
- `~/.cthg/agentes/<agente>/lock`: garantiza un solo escritor a la vez. Un candado de más de 30 s se descarta.
- `~/.cthg/agentes/<agente>/sesion/<id>.txt`: qué recibió cada sesión y hasta dónde se guardó. Se borra a los 7 días.
