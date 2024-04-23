# Project Template

Repository template for software projects.

**Steps to adapt this template.**

1. Change the link in [bug-report-form.yml](/.github/ISSUE_TEMPLATE/bug-report-form.yml) to point to your Code of Conduct
1. Change the link in [config.yml](/.github/ISSUE_TEMPLATE/config.yml) to point to your dicussions-page
1. Adapt the [release please action](/.github/workflows/release-please.yml)
1. Adapt the Definition of Done in the [pull request template](/.github/pull_request_template.md)
1. Change the contact details in the [Code of Conduct](/CODE_OF_CONDUCT#enforcement)
1. Create and link your own wiki to this project repository using the [wiki-repo-template](https://github.com/mhatzl/wiki-repo-template)
1. Adapt the [contributing](/CONTRIBUTING) section
1. Optional: Change the license of the project
1. Adapt this README

## Wiki Template

This project template works best in combination with the [wiki-repo-template](https://github.com/mhatzl/wiki-repo-template).
The wiki should be used to document development decisions and requirements.

## Template Placeholders

In this template, many sections include placeholder text to provide some guidance of what the section should be about.
These placeholders are inside `{{ }}` blocks.

Some sections also include example content that is given inside `[[ ]]` blocks.

## GitHub Actions

This template already contains the following GitHub actions:

- `release-please` ... Uses Google's [release please](https://github.com/google-github-actions/release-please-action) action to generate releases automatically

  **Note:** You should adapt this action according to your needs.

## GitHub Issue/PR Labels

Repositories created by GitHub templates do not adopt the issue/PR labels set in the template.
Instead, each one must be copied manually, which is unfortunate, but must only be done once.

**Below are the [labels](https://github.com/mhatzl/project-repo-template/labels) defined in the project template:**

- `blocked` ... Marks this issue/PR that it is blocked by another issue/PR (Color: `#F83A55`)
- `blocking` ... Marks this issue/PR that it is blocking other issues/PRs (Color: `#321575`)
- `declined` ... Marks this issue/PR as being closed without implementation (Color: `#ffffff`)
- `forces-major-bump` ... Fixing this issue/merging this PR introduces BREAKING CHANGES (Color: `#C23FB0`)
- `good-first-issue` ... Marks this issue that it is good for new contributors (Color: `#8AE5A7`)
- `help-needed` ... Assignee of this issue/PR needs help (Color: `#3DD15A`)
- `high-prio` ... Marks this issue/PR that it requires immediate attention (Color: `#D71700`)
- `hotfix` ... Marks this PR as a fix of a critical bug (Color: `#C9959E`)
- `low-prio` ... Marks this issue/PR as less important (Color: `#88BBB8`)
- `req-missing-wiki-entry` ... This REQ issue is not yet documented in the wiki (Color: `#6E5DB7`)
- `req-ready` ... Marks this REQ issue as being ready for implementation (Color: `#0487CA`)
- `waiting-on-assignee` ... Issue/PR author or reviewer is awaiting response from assignee (Color: `#FEF2C0`)
- `waiting-on-author` ... Assignee or reviewer is awaiting response from issue/PR author (Color: `#463F12`)
- `waiting-on-reviewer` ... Author or assignee is awaiting response from reviewer (Color: `#E6A2AE`)

## GitHub Settings

Repository settings are not adopted from templates, and must be set manually.
Below are the recommended settings that work well with this project template.

**General Settings:**

- Enable wiki and discussions features
- Only allow squash merging for pull requests
- Automatically delete head branches

**GitHub Actions Settings:**

- Allow GitHub Actions to create and approve pull requests

# License

MIT Licensed
