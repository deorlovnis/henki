#!/bin/sh
git log --oneline "$(git describe --tags --abbrev=0)"..HEAD
