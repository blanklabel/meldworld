package model

import (
	"github.com/gorilla/websocket"
	"github.com/satori/go.uuid"
)

// Player construct to simplify dealing with that dude
type Player struct {
	WSconn *websocket.Conn
	PlayerInfo
}

// General information about a player
type PlayerInfo struct {
	ModelType
	ID   string
	Name string
}

// Just easy way of returning a UUID for a player
func NewPlayer(c *websocket.Conn) *Player {
	u := uuid.NewV4().String()
	p := Player{
		WSconn: c,
		PlayerInfo: PlayerInfo{
			ModelType: ModelType{MsgType: PLAYERINFO},
			ID:        u,
			// Name:
		}}
	return &p
}
