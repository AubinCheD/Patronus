# Patronus

Agents autonomes et tâches récurrentes pour le développeur : des gardiens qui surveillent, préparent
et proposent, sans jamais agir seuls sur ce qui compte.

## Nom

Nom de travail : **Patronus**. Propositions, à choisir plus tard :

| Nom | Origine |
|---|---|
| **Patronus** | Le gardien invoqué dans Harry Potter : il veille et protège, comme un agent qui surveille. |
| Golem | Créature de la tradition juive, façonnée pour accomplir des tâches à la place de son créateur. |
| Gargoyle | Les gargouilles veillent sur les toits des cathédrales : des sentinelles immobiles. |

Les noms inspirés de Harry Potter (comme Fawkes) conviennent à un projet personnel ; pour une
diffusion commerciale, ils poseraient une question de marque, et Golem ou Gargoyle seraient plus
sûrs.

## Mission

Lancer, à heure fixe ou sur un événement, des agents (sessions Claude Code non interactives ou
scripts) qui préparent un travail : revue, tri, correctif, rapport. Les rendre visibles, bornés et
réversibles : chaque résultat arrive dans une boîte de résultats, et rien ne part vers l'extérieur
sans validation.

## Périmètre

**Fait** :
- déclencheurs : heure, fichier modifié, nouveau commit ou branche distante, PR ouverte ou commentée,
  CI en échec, motif dans la sortie d'un terminal, service arrêté, à la main ;
- actions : `claude -p` dans un worktree dédié, script, ou session interactive ouverte dans Fawkes ;
- boîte de résultats : résumé, diff, liens, coût, actions proposées ;
- garde-fous : budget, durée, outils permis, journal, arrêt.

**Ne fait pas** :
- pousser, fusionner, déployer ou publier sans validation explicite ;
- remplacer la CI ou un orchestrateur de production ;
- garder des secrets dans les consignes.

## Place dans l'écosystème

- Application autonome, et futur module de la suite avec Fawkes (terminaux) et DragonTree (git).
- Même pile : Rust, eframe et egui 0.37, rendu glow.
- État dans son propre dossier : passer de l'application autonome à la suite ne demande aucune
  migration.
- Reçoit ses demandes par le contrat local commun aux applications (canal nommé réservé au compte).
- Les sessions qu'il lance peuvent s'ouvrir dans Fawkes, groupe « Agents », pour reprendre la main.

## Premières étapes

1. Format d'une tâche (fichier lisible, versionnable) et validation.
2. Exécution manuelle d'une tâche `claude -p` dans un worktree, résultat dans la boîte.
3. Déclencheur horaire, puis déclencheurs git.
4. Garde-fous (budget, durée, outils) appliqués et testés.
5. Ouverture d'une exécution dans Fawkes.

Détail : [`docs/spec.md`](docs/spec.md).

## État

Premier jet (octobre 2026) :
- tâches en fichiers JSON (`<données>\taches\<id>.json`), éditables dans l'application ;
- déclencheurs : heure (jours choisis, rattrapage facultatif), intervalle, fichier modifié, branche
  distante qui avance (`git ls-remote`, sans rien écrire ; la tâche reçoit la liste des nouveaux
  commits, dans sa consigne ou `PATRONUS_BRANCHE`, `PATRONUS_COMMIT_AVANT`,
  `PATRONUS_COMMIT_APRES`), à la main (bouton, palette, `patronus lancer <tâche>`) ;
- actions : script PowerShell, cmd ou bash ; session Claude non interactive (`claude -p`) :
  - lecture seule par défaut ; dès qu'elle peut écrire, dans un worktree dédié (jamais dans la copie
    de travail) ;
  - mode restreint de Claude Code (`--restricted` : réglages de l'utilisateur et du dépôt ignorés,
    outils de fichiers confinés au dossier de travail), seuls les outils intégrés de la tâche
    (`--tools`), aucun serveur MCP d'ailleurs ;
  - push, fusion, publication, commentaires de PR interdits ; en plus, toute adresse de push est
    réécrite vers un protocole inexistant dans l'environnement de la session ;
  - budget obligatoire (`--max-budget-usd`, et estimation de Patronus en cours de route), tours
    bornés (`--max-turns`) ;
  - budget mensuel facultatif (réglages) : une session qui risquerait de le dépasser ne part pas ;
    coût du mois dans la barre d'état ;
  - résumé, coût « équivalent API », appels refusés, transcription et diff dans la boîte ;
  - Claude Code récent requis (`claude --help` doit citer `--restricted`) ;
- garde-fous : durée maximale, une exécution à la fois par tâche (un départ en attente au plus),
  arrêt par tâche ou global, journal complet, tâche invalide ou soupçonnée de contenir un secret
  refusée ;
- boîte de résultats : statut, résumé, journal, filtres, non lus, archivage ; elle survit au
  redémarrage ; une exécution manquée (Patronus fermé) y est signalée, une exécution coupée par un
  arrêt de Patronus y apparaît « interrompue » ;
- actions sur un résultat, toujours après confirmation : appliquer le diff d'une session dans la
  copie de travail (sans commit), supprimer son worktree, reprendre la session (`claude --resume`,
  dans Windows Terminal ou PowerShell), copier le résumé ;
- fermer la fenêtre pendant une exécution, ou avec un brouillon non enregistré, demande confirmation ;
  une exécution coupée ainsi est « interrompue : Patronus a été fermé » ;
- lancement à l'ouverture de session (réglages, décoché par défaut) : clé `Run` du compte, fenêtre
  réduite (`--reduit`) ;
- notification Windows à la fin, thèmes, raccourcis réglables.

### Premier essai réel d'une session Claude

Les tests n'emploient qu'un faux `claude` (`faux-claude`) : le premier vrai lancement se fait à la
main, et il est facturé sur le compte Claude du poste (quelques centimes au plus).

1. *+ Nouvelle tâche*, *Dépôt* : un dépôt sans enjeu, puis le modèle *Essai Claude (lecture seule)* :
   lecture seule, sans worktree, 0,10 $ et 5 tours au plus. *Enregistrer*.
2. *▶*. Pendant la session, le journal se remplit (réponses de Claude, outils appelés).
3. À la fin, vérifier :
   - le résumé (la réponse de Claude) et le coût « équivalent API », exact (sans « ≈ ») ;
   - la ligne de commande dans le journal : `--restricted`, `--tools Read,Grep,Glob`,
     `--max-budget-usd 0.1`, `--max-turns 5` ;
   - `sortie.jsonl` (flux brut) et `consigne.txt` dans le dossier de l'exécution (*Ouvrir le dossier*).
4. Les arrêts : budget à 0,01 $ (« arrêtée par Claude Code : budget atteint ») ; 1 tour (« nombre de
   tours ») ; *■* pendant la session (« arrêtée à la main », coût estimé « ≈ »).
5. Une version de Claude Code qui ne connaît pas `--restricted` fait échouer l'exécution, et le
   journal le dit : mettre Claude Code à jour.

### Fenêtre fermée

Par défaut, une tâche ne tourne que lorsque Patronus est ouvert (les échéances manquées sont signalées
à l'ouverture, ou rattrapées si la tâche le demande). La case *Tourner aussi quand Patronus est
fermé* (`"hors_application": true`) inscrit ses échéances horaires (heure, intervalle) au
Planificateur de tâches de Windows, dans le dossier `\Patronus\` (`\Patronus-<empreinte>\` pour un
autre dossier de données), sans droit administrateur, seulement quand la session est ouverte. Le
Planificateur lance alors `patronus.exe --donnees <dossier> planifie <tâche>` :

- fenêtre ouverte, il ne fait rien (elle tient ses échéances) ;
- échéance déjà tenue, il ne fait rien ;
- sinon la tâche part, sans fenêtre, et son résultat arrive dans la boîte ;
- chaque passage est noté dans `planificateur.log`.

Les tâches Windows sont mises à jour à l'ouverture et à chaque modification des tâches (exécutable
déplacé compris) ; décocher la case, ou supprimer la tâche, retire la tâche Windows. Fichiers modifiés
et branches distantes demandent Patronus ouvert.

## Compilation

Prérequis : Rust stable (toolchain `x86_64-pc-windows-gnu`), composant `llvm-tools`.

```powershell
tools\setup.ps1            # une fois par clone : dlltool et .cargo\config.toml (non versionné)
cargo build --release      # → target\release\patronus.exe
cargo test
```

## Lancer et vérifier

```powershell
target\release\patronus.exe                  # la fenêtre
target\release\patronus.exe --reduit         # la fenêtre, réduite dans la barre des tâches
target\release\patronus.exe lancer <tâche>   # exécute une tâche sans fenêtre, résultat dans la boîte
# depuis un terminal, pour attendre la fin et lire le code de retour :
Start-Process -Wait -NoNewWindow target\release\patronus.exe -ArgumentList 'lancer','<tâche>'
```

`lancer` ne fait que lancer la tâche demandée (ni échéances, ni surveillance) ; code de retour 0
(réussie), 1 (autre statut), 2 (tâche inconnue), 3 (déjà en cours, ici ou dans la fenêtre). Le
résultat arrive dans la boîte, que la fenêtre relit toutes les 5 s. `--donnees <dossier>` choisit le
dossier de données.

1. *+ Nouvelle tâche* : un nom, un déclencheur (*+ Heure*, *+ Intervalle* ou *+ Fichier modifié*),
   une commande (par exemple `Get-Date` en PowerShell), *Enregistrer*.
2. *▶* sur la tâche, ou attendre son échéance, ou modifier un fichier du dossier surveillé.
3. Le résultat arrive dans la *Boîte de résultats* (compteur des non lus dans le titre), avec son
   journal. Fermer et rouvrir : il y est toujours.

Données : `%APPDATA%\Patronus`, ou le dossier désigné par `PATRONUS_DATA_DIR` (essais, profils
séparés ; une instance par dossier). Pour les essais d'interface, `PATRONUS_CAPTURE=<image.png>`
enregistre une capture de la fenêtre après 3 s (`PATRONUS_CAPTURE_DELAI`, en secondes) puis ferme ;
`PATRONUS_CAPTURE_VUE=editeur|reglages|palette` choisit la vue, `PATRONUS_ZOOM=2` agrandit l'interface.

| Fichier | Contenu |
|---|---|
| `taches\<id>.json` | une tâche (format : `docs/spec.md` §8) ; une tâche supprimée devient `<id>.json.supprimee` |
| `executions\<id>\run.json` | une entrée de la boîte ; `journal.txt` à côté |
| `planning.json` | dernière surveillance des échéances (pour signaler les manquées), dernier commit vu des branches distantes |
| `planificateur.log` | passages du Planificateur de Windows (fenêtre fermée) |
| `couts\<année-mois>.txt` | coût de chaque session Claude du mois (budget mensuel) |
| `verrous\`, `worktrees\` | verrou de chaque tâche en cours ; worktrees des sessions Claude |
| `state.json` | réglages (thème, raccourcis, rétention), versionnés |

Le script reçoit `PATRONUS_TACHE`, `PATRONUS_DECLENCHEUR`, `PATRONUS_FICHIERS` (fichiers modifiés, un
par ligne, 100 au plus) et `PATRONUS_FICHIERS_LISTE` (chemin de la liste complète). Une session Claude
reçoit sa consigne sur l'entrée standard (`consigne.txt` dans le dossier de l'exécution), avec au plus
50 fichiers modifiés cités.

Journal des décisions : [`docs/journal.md`](docs/journal.md). Contrat proposé avec les autres
modules : [`docs/contrat-local.md`](docs/contrat-local.md).
