# Journal des échanges et des décisions

Ce qui a été demandé, choisi ou décidé pendant le développement, du plus ancien au plus récent.
« Provisoire » : choix fait sans l'utilisateur, à confirmer.

## 21/10/2026 — démarrage du premier jet

**Demande** : construire une première version utile de bout en bout, par étapes (un commit par
étape, tests de la logique), sans rien pousser ni toucher aux autres dépôts.

**Pilotage** : l'utilisateur a confié le suivi à une autre session (« pilote »), qui répond aux
questions de choix jusqu'au premier jet ; il reprendra la main ensuite.

**Premier jet attendu** (pilote) : l'exécutable se lance ; on y définit un gardien (déclencheur
heure ou fichier, action script) ; il s'exécute et son résultat arrive dans la boîte de résultats,
visible dans l'interface ; l'historique survit à un redémarrage ; `cargo test` au vert ; le README
explique comment le lancer. Les sessions Claude non interactives viennent après les scripts, testées
avec un faux exécutable.

**Choix (provisoires, validés par le pilote)** :
- **Hors application** : les tâches ne tournent que si Patronus est ouvert. Les exécutions manquées
  sont signalées à l'ouverture (une entrée « manquée » dans la boîte), ou rattrapées si la tâche le
  demande. Service ou tâche planifiée Windows : plus tard.
- **Déclencheurs du premier jet** : heure (et intervalle), fichier modifié, à la main. Branche
  distante ensuite si le temps le permet.
- **Structure** : workspace de crates, `patronus-core` (logique sans interface, testable) et
  `patronus-app` (bibliothèque + exécutable `patronus`), comme DragonTree.
- **Coût** : tokens et montant « équivalent API » rapporté par Claude ; le budget d'une tâche
  s'applique à cet équivalent.

**Choix d'implémentation (miens)** :
- Format des tâches : celui de `docs/spec.md` §8, champs en français ; un déclencheur se reconnaît à
  sa clé (`heure`, `toutes_les_minutes`, `fichiers`) ; les champs inconnus sont gardés.
- Une tâche supprimée est mise de côté (`<id>.json.supprimee`), pas effacée.
- Une commande ou une consigne qui ressemble à un secret (jeton connu, `password=`…) rend la tâche
  invalide.
- Compilation avec la toolchain gnu : même préparation que Fawkes (`tools\setup.ps1`, recopié).

## 21/10/2026 — premier jet

**Pilote** : faire l'interface minimale tôt, juste après l'exécution des scripts ; vérifier
l'interface par une capture faite par l'application elle-même (l'écran du poste est en veille).
Fait : `PATRONUS_CAPTURE`, avec un dossier de données isolé (`PATRONUS_DATA_DIR`).

**Choix d'implémentation (miens, provisoires)** :
- Un fichier modifié par la tâche pendant qu'elle tourne ne la redéclenche pas (sinon une tâche qui
  écrit dans le dossier qu'elle surveille bouclerait) ; c'est noté dans son journal.
- Les départs manqués d'un même déclencheur sont regroupés en une seule entrée « manquée » (ou un
  seul rattrapage).
- Une exécution laissée « en cours » par un arrêt de Patronus devient « interrompue » au démarrage.
- Rétention : les résultats lus ou archivés de plus de 30 jours sont supprimés (réglable) ; les non
  lus restent.
- Notification Windows : bulle d'une icône de zone de notification, ajoutée à la première
  notification.
- `patronus lancer <tâche>` exécute la tâche dans le processus appelant, sans passer par la fenêtre
  ouverte : en attendant le canal nommé, deux exécutions de la même tâche restent possibles si on
  lance en ligne de commande pendant qu'elle tourne dans la fenêtre.
- Contrat local : proposition dans `docs/contrat-local.md`.

## 21/10/2026 — action « session Claude non interactive »

**Pilote** : finir l'action Claude, cœur de Patronus ; tests uniquement avec un faux exécutable,
aucun vrai `claude -p` pendant le développement ; budget vérifié avant le lancement et pendant
l'exécution, arrêt propre ; résultat (texte, coût, worktree) dans la boîte.

**Fait** :
- `claude -p <consigne> --output-format stream-json --verbose --allowedTools … --disallowedTools …
  --max-budget-usd …` dans le dépôt, ou dans un worktree détaché de la branche demandée
  (`<données>\worktrees\<exécution>`) ; la copie de travail de l'utilisateur n'est jamais touchée.
- Outils : lecture seule (Read, Grep, Glob) si la tâche n'en liste aucun ; `git push`, `git merge`,
  `gh pr merge`, `gh release`, `npm publish`, `cargo publish` toujours interdits.
- Budget : obligatoire pour une session Claude (une tâche sans budget est invalide et ne part pas) ;
  passé à Claude Code ; et, pendant l'exécution, le coût annoncé dans le flux est surveillé : au-delà,
  arrêt de l'arbre de processus, entrée « arrêtée : budget atteint ».
- Tours : `--max-turns` n'apparaît pas dans l'aide de la CLI installée ; Patronus compte les messages
  de Claude dans le flux et arrête au-delà de `tours_max` (approximation : un message par appel
  d'outil).
- Boîte : réponse de Claude en résumé, tokens et coût « équivalent API », transcription lisible dans
  le journal (flux brut dans `sortie.jsonl`), identifiant de session gardé ; le worktree est gardé
  avec `diff.patch` s'il a produit quelque chose, supprimé sinon.
- Essais : `faux-claude` (binaire d'essai de `patronus-core`), désigné par `PATRONUS_CLAUDE`.

## Pistes après le premier jet

1. **Symboles d'état** : ils s'affichaient en carrés vides (glyphes absents de la police). Corrigé
   pendant le premier jet : police d'emoji monochrome d'egui pour ▶ ■ ✏, pastilles d'état dessinées.
   À revérifier à l'œil sur l'écran réel (les captures étaient réduites).
2. **Branche distante** : déclencheur « `origin/<branche>` avance » par `git fetch` périodique, sans
   écrire dans le dépôt de travail ; la consigne reçoit le journal des nouveaux commits.
3. **Application fermée** : tâche planifiée de Windows qui lance `patronus lancer <tâche>` aux
   échéances ; l'interface retrouve les résultats dans la boîte. À trancher : une tâche planifiée par
   tâche, ou un service.
4. **Premier essai réel de l'action Claude**, par l'utilisateur : une tâche en lecture seule, petit
   budget, sur un dépôt sans enjeu ; vérifier le résumé, le coût affiché, le journal, et que le
   budget et les tours arrêtent bien une vraie session.

Autres pistes : *Appliquer le diff* dans la copie de travail (après confirmation), *Reprendre dans
Fawkes* (session gardée, attend le contrat local), canal nommé (`docs/contrat-local.md`), binaire
`faux-claude` à sortir de la distribution, tâches propres à un dépôt (`.patronus/`), enchaînement de
tâches.

## 21/10/2026 (soir) — passe complète

**Demande de l'utilisateur** (effort xhigh) : refaire une passe complète, traiter les quatre points
remontés, ajouter des fonctionnalités utiles, ajouter superpowers et une feuille de route. Pause de
deux heures à 19 h 10 (limite d'usage), reprise à 21 h. Format repris de Menagerie : relecture (A),
points remontés (B), fonctionnalités (C), dans `docs/feuille-de-route.md`, et un plan dans
`docs/superpowers/plans/`.

**Relecture** : confiée à un relecteur indépendant (sans l'historique de la session), confrontée à la
documentation de Claude Code (agent de documentation) et à la CLI installée (2.1.306). Deux défauts
critiques démontrés par essai (boutons de la liste des tâches morts ; tâche qui se relance sans fin
sur ses propres écritures), dix importants, douze mineurs : tous retenus (A1 à A13).

**Décisions (miennes, provisoires)** :
- **A2** : un fichier dont la date de modification tombe pendant l'exécution d'une tâche (ou dans la
  seconde qui suit) lui est attribué et ne la relance pas ; c'est noté dans son journal. Limite
  assumée : une modification de l'utilisateur pendant l'exécution est ignorée de la même façon. Un
  même changement signalé deux fois par la surveillance ne relance qu'une fois.
- **Garde-fou contre les push** : une règle d'outil comme `Bash(git push:*)` laisse passer
  `git -C . push` (documentation de Claude Code). En plus des règles, la session reçoit une
  configuration git (`url.patronus-push-interdit:.pushInsteadOf` vide, par `GIT_CONFIG_COUNT`) qui
  réécrit toute adresse de push vers un protocole inexistant. Vérifié sans rien pousser
  (`git remote get-url --push`) : un essai avec un vrai `git push`, même vers un dépôt jetable local,
  a été refusé par le système de permissions de la session, et je ne l'ai pas contourné.
- **Commit `906c5ca` incomplet** : une erreur de `git add` (chemin de l'ancien `app.rs`) l'a privé du
  moteur, des tests et de la feuille de route ; seul, il ne compile pas. Complété par le commit
  suivant plutôt que réécrit (pas de `--amend` dans ce dépôt) ; un `git bisect` peut le sauter.
