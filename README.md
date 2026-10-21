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
