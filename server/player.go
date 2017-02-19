package main

import (
	"github.com/gorilla/websocket"
	"github.com/satori/go.uuid"
)

// Player construct to simplify dealing with that dude
type Player struct {
	conn *websocket.Conn
	PlayerMeta
}

// General information about a player
type PlayerMeta struct {
	ID   string
	Name string
}

// Just easy way of returning a UUID for a player
func NewPlayer(c *websocket.Conn) *Player {
	u := uuid.NewV4().String()
	p := Player{
		conn: c,
		PlayerMeta: PlayerMeta{
			ID: u,
			// Name:
		}}
	return &p
}
